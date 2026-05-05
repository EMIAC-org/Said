//! Diagnostic: try every known macOS method to read the focused text field.
//!
//! Run from the workspace root:
//!   cargo run --example test_ax_methods -p voice-polish-paster
//!
//! You have 5 seconds after "Switching focus in…" to switch to the app/field
//! you want to test.  All 5 methods print their result automatically.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{CStr, CString, c_void};
use std::process::Command;
use std::thread;
use std::time::Duration;

const CF_UTF8: u32 = 0x0800_0100;
// kAXValueCFRangeType = 4  (AXValue.h)
const K_AX_VALUE_CF_RANGE_TYPE: u32 = 4;

#[repr(C)]
struct CFRange {
    location: i64,
    length: i64,
}

// ── FFI ───────────────────────────────────────────────────────────────────────

#[link(name = "CoreFoundation", kind = "framework")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    // CoreFoundation strings
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const i8,
        encoding: u32,
    ) -> *mut c_void;
    fn CFStringGetCString(s: *const c_void, buf: *mut i8, buf_size: i64, encoding: u32) -> bool;
    fn CFStringGetLength(s: *const c_void) -> i64;
    fn CFGetTypeID(cf: *const c_void) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFRelease(cf: *mut c_void);

    // CFBoolean
    static kCFBooleanTrue: *const c_void;

    // CFNumber
    fn CFNumberGetValue(number: *const c_void, the_type: i32, value_ptr: *mut c_void) -> bool;

    // CFArray
    fn CFArrayGetCount(array: *const c_void) -> i64;
    fn CFArrayGetValueAtIndex(array: *const c_void, idx: i64) -> *const c_void;

    // Accessibility
    fn AXIsProcessTrusted() -> bool;
    fn AXUIElementCreateSystemWide() -> *mut c_void;
    fn AXUIElementCopyAttributeValue(
        el: *const c_void,
        attr: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn AXUIElementSetAttributeValue(
        el: *const c_void,
        attr: *const c_void,
        value: *const c_void,
    ) -> i32;
    fn AXUIElementGetPid(el: *const c_void, pid: *mut i32) -> i32;
    fn AXUIElementCopyAttributeNames(el: *const c_void, out: *mut *mut c_void) -> i32;
    fn AXUIElementCopyParameterizedAttributeValue(
        el: *const c_void,
        attr: *const c_void,
        param: *const c_void,
        out: *mut *mut c_void,
    ) -> i32;
    fn AXValueCreate(the_type: u32, value_ptr: *const c_void) -> *mut c_void;
}

// ── Small helpers ─────────────────────────────────────────────────────────────

unsafe fn cf_str(s: &str) -> *mut c_void {
    let cs = CString::new(s).unwrap();
    unsafe { CFStringCreateWithCString(std::ptr::null(), cs.as_ptr(), CF_UTF8) }
}

unsafe fn cfstring_to_rust(cf: *const c_void) -> Option<String> {
    if cf.is_null() {
        return None;
    }
    if unsafe { CFGetTypeID(cf) != CFStringGetTypeID() } {
        return None;
    }
    let n = unsafe { CFStringGetLength(cf) };
    if n < 0 {
        return None;
    }
    let size = (n * 4 + 1) as usize;
    let mut buf: Vec<i8> = vec![0; size];
    if unsafe { CFStringGetCString(cf, buf.as_mut_ptr(), size as i64, CF_UTF8) } {
        Some(
            unsafe { CStr::from_ptr(buf.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        None
    }
}

unsafe fn ax_get(el: *const c_void, attr: &str) -> Option<*mut c_void> {
    let key = unsafe { cf_str(attr) };
    let mut out: *mut c_void = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyAttributeValue(el, key, &mut out) };
    unsafe { CFRelease(key) };
    if err == 0 && !out.is_null() {
        Some(out)
    } else {
        None
    }
}

unsafe fn ax_get_string(el: *const c_void, attr: &str) -> Result<String, &'static str> {
    match unsafe { ax_get(el, attr) } {
        None => Err("attribute missing / nil"),
        Some(cf) => {
            let s = unsafe { cfstring_to_rust(cf as *const _) };
            unsafe { CFRelease(cf) };
            s.ok_or("not a string")
        }
    }
}

unsafe fn ax_set_bool(el: *const c_void, attr: &str) -> i32 {
    let key = unsafe { cf_str(attr) };
    let err = unsafe { AXUIElementSetAttributeValue(el, key, kCFBooleanTrue) };
    unsafe { CFRelease(key) };
    err
}

unsafe fn dump_attr_names(el: *const c_void) -> Vec<String> {
    let mut out: *mut c_void = std::ptr::null_mut();
    if unsafe { AXUIElementCopyAttributeNames(el, &mut out) } != 0 || out.is_null() {
        return vec![];
    }
    let n = unsafe { CFArrayGetCount(out as *const _) };
    let mut names = Vec::new();
    for i in 0..n {
        let item = unsafe { CFArrayGetValueAtIndex(out as *const _, i) };
        if let Some(s) = unsafe { cfstring_to_rust(item) } {
            names.push(s);
        }
    }
    unsafe { CFRelease(out) };
    names
}

// ── Method 1: direct AXValue ──────────────────────────────────────────────────

unsafe fn method1(_app: *const c_void, el: *const c_void) -> Result<String, String> {
    unsafe { ax_get_string(el, "AXValue") }.map_err(|e| e.to_string())
}

// ── Method 2: AXEnhancedUserInterface + AXManualAccessibility unlock ──────────

unsafe fn method2(app: *const c_void, _el: *const c_void) -> Result<String, String> {
    let r1 = unsafe { ax_set_bool(app, "AXEnhancedUserInterface") };
    let r2 = unsafe { ax_set_bool(app, "AXManualAccessibility") };
    println!("   unlock: AXEnhancedUserInterface err={r1}, AXManualAccessibility err={r2}");
    thread::sleep(Duration::from_millis(120));

    // Re-fetch element after unlock (tree may have rebuilt)
    let el2 = unsafe { ax_get(app, "AXFocusedUIElement") };
    let Some(el2) = el2 else {
        return Err("AXFocusedUIElement nil after unlock".to_string());
    };
    let result = unsafe { ax_get_string(el2 as *const _, "AXValue") }.map_err(|e| e.to_string());
    unsafe { CFRelease(el2) };
    result
}

// ── Method 3: AXSelectedText ──────────────────────────────────────────────────

unsafe fn method3(_app: *const c_void, el: *const c_void) -> Result<String, String> {
    unsafe { ax_get_string(el, "AXSelectedText") }.map_err(|e| e.to_string())
}

// ── Method 4: AXNumberOfCharacters + AXStringForRange ────────────────────────

unsafe fn method4(_app: *const c_void, el: *const c_void) -> Result<String, String> {
    // kCFNumberSInt64Type = 4
    let n_cf = unsafe { ax_get(el, "AXNumberOfCharacters") }
        .ok_or_else(|| "AXNumberOfCharacters missing".to_string())?;

    let mut char_count: i64 = 0;
    let ok = unsafe {
        CFNumberGetValue(
            n_cf as *const _,
            4,
            &mut char_count as *mut i64 as *mut c_void,
        )
    };
    unsafe { CFRelease(n_cf) };
    if !ok || char_count <= 0 {
        return Err(format!("AXNumberOfCharacters={char_count}"));
    }
    println!("   AXNumberOfCharacters = {char_count}");

    let range = CFRange {
        location: 0,
        length: char_count,
    };
    let range_val = unsafe {
        AXValueCreate(
            K_AX_VALUE_CF_RANGE_TYPE,
            &range as *const _ as *const c_void,
        )
    };
    if range_val.is_null() {
        return Err("AXValueCreate returned null".to_string());
    }

    let attr = unsafe { cf_str("AXStringForRange") };
    let mut out: *mut c_void = std::ptr::null_mut();
    let err = unsafe { AXUIElementCopyParameterizedAttributeValue(el, attr, range_val, &mut out) };
    unsafe { CFRelease(attr) };
    unsafe { CFRelease(range_val) };

    if err != 0 || out.is_null() {
        return Err(format!("AXStringForRange err={err}"));
    }
    let result = unsafe { cfstring_to_rust(out as *const _) };
    unsafe { CFRelease(out) };
    result.ok_or_else(|| "not a string".to_string())
}

// ── Method 5: AX tree traversal ──────────────────────────────────────────────

unsafe fn find_text_in_children(el: *const c_void, depth: usize) -> Option<String> {
    if depth > 8 {
        return None;
    }

    if let Some(val) = unsafe { ax_get(el, "AXValue") } {
        let s = unsafe { cfstring_to_rust(val as *const _) };
        unsafe { CFRelease(val) };
        if let Some(ref text) = s {
            if !text.is_empty() {
                return s;
            }
        }
    }

    let children_cf = unsafe { ax_get(el, "AXChildren") };
    let Some(children) = children_cf else {
        return None;
    };

    let n = unsafe { CFArrayGetCount(children as *const _) };
    for i in 0..n {
        let child = unsafe { CFArrayGetValueAtIndex(children as *const _, i) };
        if let Some(text) = unsafe { find_text_in_children(child, depth + 1) } {
            unsafe { CFRelease(children) };
            return Some(text);
        }
    }
    unsafe { CFRelease(children) };
    None
}

unsafe fn method5(app: *const c_void, el: *const c_void) -> Result<String, String> {
    if let Some(text) = unsafe { find_text_in_children(el, 0) } {
        return Ok(text);
    }
    let win = unsafe { ax_get(app, "AXFocusedWindow") };
    if let Some(w) = win {
        let result = unsafe { find_text_in_children(w, 0) };
        unsafe { CFRelease(w) };
        if let Some(text) = result {
            return Ok(text);
        }
    }
    Err("no text found in AX tree".to_string())
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn pbpaste() -> String {
    Command::new("pbpaste")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn print_result(label: &str, result: Result<String, String>) {
    match result {
        Ok(v) if v.is_empty() => println!("⚠️  returned empty string (nothing to read)"),
        Ok(v) => println!("✅ {} chars: {:?}", v.len(), truncate(&v, 100)),
        Err(e) => println!("❌ {e}"),
    }
    let _ = label;
}

fn main() {
    println!("┌─────────────────────────────────────────────────────────┐");
    println!("│       AX Field Diagnostic — 5 Methods                   │");
    println!("│                                                         │");
    println!("│  Type some text in any app. You have 5 seconds to       │");
    println!("│  switch focus there AFTER this prompt.                  │");
    println!("└─────────────────────────────────────────────────────────┘");

    for i in (1..=5u8).rev() {
        println!("  Switching focus in {i}s…");
        thread::sleep(Duration::from_secs(1));
    }
    println!("\n  ── Probing now ──\n");

    {
        if !unsafe { AXIsProcessTrusted() } {
            println!("❌ Accessibility permission NOT granted.");
            println!("   System Settings → Privacy & Security → Accessibility → add this terminal");
            return;
        }
        println!("✅ Accessibility: GRANTED\n");

        // Get focused application
        let sys = unsafe { AXUIElementCreateSystemWide() };
        let app = match unsafe { ax_get(sys as *const _, "AXFocusedApplication") } {
            Some(a) => {
                unsafe { CFRelease(sys) };
                a
            }
            None => {
                unsafe { CFRelease(sys) };
                println!("❌ No focused application found");
                return;
            }
        };

        // App info
        let mut pid: i32 = -1;
        unsafe { AXUIElementGetPid(app as *const _, &mut pid) };
        let app_name = unsafe { ax_get_string(app as *const _, "AXTitle") }
            .unwrap_or_else(|_| format!("pid={pid}"));
        println!("Focused app : {app_name}  (pid={pid})\n");

        // Focused element
        let el = match unsafe { ax_get(app as *const _, "AXFocusedUIElement") } {
            Some(e) => e,
            None => {
                println!("❌ AXFocusedUIElement is nil — no text field focused?");
                unsafe { CFRelease(app) };
                return;
            }
        };

        // List available AX attributes on the element
        let attrs = unsafe { dump_attr_names(el as *const _) };
        println!("Available attributes on focused element ({}):", attrs.len());
        let relevant = [
            "AXValue",
            "AXSelectedText",
            "AXNumberOfCharacters",
            "AXStringForRange",
            "AXRole",
            "AXRoleDescription",
            "AXDescription",
            "AXPlaceholderValue",
        ];
        for r in &relevant {
            let present = if attrs.iter().any(|a| a == r) {
                "✓"
            } else {
                "✗"
            };
            println!("  {present} {r}");
        }
        if let Ok(role) = unsafe { ax_get_string(el as *const _, "AXRole") } {
            println!("  → AXRole = {role}");
        }
        println!();

        // ── Run methods ───────────────────────────────────────────────────────
        println!("══ Method 1: Direct AXValue ══════════════════════════════");
        print_result("1", unsafe { method1(app as *const _, el as *const _) });
        println!();

        println!("══ Method 2: AX tree unlock (Chrome/Electron) + AXValue ═");
        print_result("2", unsafe { method2(app as *const _, el as *const _) });
        println!();

        println!("══ Method 3: AXSelectedText ══════════════════════════════");
        print_result("3", unsafe { method3(app as *const _, el as *const _) });
        println!();

        println!("══ Method 4: AXNumberOfCharacters + AXStringForRange ═════");
        print_result("4", unsafe { method4(app as *const _, el as *const _) });
        println!();

        println!("══ Method 5: AX children tree walk ═══════════════════════");
        print_result("5", unsafe { method5(app as *const _, el as *const _) });
        println!();

        println!("══ Bonus: clipboard (pbpaste) ════════════════════════════");
        let clip = pbpaste();
        if clip.is_empty() {
            println!("⚠️  clipboard empty");
        } else {
            println!("✅ {} chars: {:?}", clip.len(), truncate(&clip, 100));
        }

        unsafe { CFRelease(el) };
        unsafe { CFRelease(app) };
    }

    println!("\n── Done ──");
}
