//! App Identity Service — turn "the app a dictation was pasted into" into a
//! renderable identity (icon now; display name + category are the next layer).
//!
//! Two public operations, both cross-platform:
//!   - `app_key_for_pid(pid)` — resolve a process id to a stable app key.
//!       · macOS  : the bundle-id (e.g. `com.google.Chrome`), via NSRunningApplication.
//!       · Windows: the full exe path, via QueryFullProcessImageNameW.
//!   - `icon_data_url(app_key)` — resolve that key to a `data:image/png;base64,…`
//!     URL you can drop straight into an <img src>. Results are cached in-process
//!     (an app's icon never changes within a session), so repeated History rows
//!     for the same app cost one native call total.
//!
//! `app_key` is exactly what we persist as `recordings.target_app`, so the
//! History page can re-resolve the icon later from the stored value alone —
//! the capture-time pid is long gone by render time.
//!
//! Design note: this is the seed of the per-app knowledge-base feature. Today it
//! only renders icons; `display_name` + `category` ("what the app is for", from
//! Spotlight `kMDItemAppStoreCategory` on macOS / version-info on Windows) land
//! on top of this same module next.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Resolve a process id to a stable per-app key (bundle-id on macOS, exe path on
/// Windows). `None` if the pid is gone or the platform can't resolve it.
pub fn app_key_for_pid(pid: i32) -> Option<String> {
    imp::app_key_for_pid(pid)
}

/// Resolve an app key to a PNG icon as a `data:image/png;base64,…` URL, cached
/// in-process. `None` if the app can't be found or has no icon.
pub fn icon_data_url(app_key: &str) -> Option<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(app_key) {
            return hit.clone(); // may be a cached `None` — don't re-probe misses
        }
    }
    let computed = imp::icon_data_url(app_key);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(app_key.to_string(), computed.clone());
    }
    computed
}

/// A resolved app identity for the UI: what the app is called, roughly what it's
/// for, and its icon. Every field is best-effort — any may be `None`.
#[derive(Clone, serde::Serialize)]
pub struct AppIdentity {
    /// The stored app key (bundle-id on macOS / exe path on Windows) — join key.
    pub key: String,
    /// Friendly display name (e.g. "Google Chrome").
    pub name: Option<String>,
    /// Coarse category / "what it does" (e.g. "Developer Tools", "Productivity").
    pub category: Option<String>,
    /// Icon as a `data:image/png;base64,…` URL.
    pub icon: Option<String>,
}

/// Resolve everything the UI needs about an app key (name + category + icon),
/// cached in-process so the Insights aggregation only pays the native cost once
/// per distinct app.
pub fn describe(app_key: &str) -> AppIdentity {
    static CACHE: OnceLock<Mutex<HashMap<String, AppIdentity>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(hit) = guard.get(app_key) {
            return hit.clone();
        }
    }
    let identity = AppIdentity {
        key: app_key.to_string(),
        name: imp::display_name(app_key),
        category: imp::category(app_key).map(prettify_category),
        icon: icon_data_url(app_key),
    };
    if let Ok(mut guard) = cache.lock() {
        guard.insert(app_key.to_string(), identity.clone());
    }
    identity
}

/// Normalize a category token to a human label. Spotlight already returns pretty
/// labels ("Developer Tools") which pass through; the reverse-DNS Info.plist form
/// (`public.app-category.developer-tools`) is title-cased from its last segment.
fn prettify_category(raw: String) -> String {
    let raw = raw.trim().to_string();
    if raw.contains(' ') && !raw.contains('.') {
        return raw;
    }
    let tail = raw.rsplit('.').next().unwrap_or(&raw);
    tail.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Standard base64 (RFC 4648) with padding — small enough to inline rather than
/// pull a crate into the desktop shell just for icon URLs. Shared with `favicon`.
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ── macOS ───────────────────────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, CString};
    use std::path::Path;

    use core_foundation::base::TCFType;
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::CFURL;
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    // Spotlight metadata (CoreServices) — reads the live index, no subprocess.
    // MDItem attribute keys ARE their own literal strings, so we pass
    // CFString::new("kMDItemAppStoreCategory") instead of linking the constant.
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn MDItemCreateWithURL(
            allocator: *const std::ffi::c_void,
            url: *const std::ffi::c_void,
        ) -> *const std::ffi::c_void;
        fn MDItemCopyAttribute(
            item: *const std::ffi::c_void,
            name: *const std::ffi::c_void,
        ) -> *const std::ffi::c_void;
    }
    unsafe extern "C" {
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    // The `cocoa` crate's `id`/`NSRect` are blanket-deprecated (→ objc2). The
    // rest of this shell talks to AppKit via raw `objc` (see permissions.rs), so
    // do the same: a bare Objective-C object pointer alias + core-graphics geometry
    // (NSRect *is* CGRect on 64-bit macOS, so the ABI matches).
    #[allow(non_camel_case_types)]
    type id = *mut Object;
    const NIL: id = std::ptr::null_mut();

    /// NSBitmapImageFileTypePNG.
    const NS_PNG_FILE_TYPE: u64 = 4;
    /// Icon side (points) requested from the image; a 2× rep gives crisp 64px.
    const ICON_SIDE: f64 = 32.0;

    pub fn app_key_for_pid(pid: i32) -> Option<String> {
        if pid <= 0 {
            return None;
        }
        unsafe {
            let cls = Class::get("NSRunningApplication")?;
            let app: id = msg_send![cls, runningApplicationWithProcessIdentifier: pid];
            if app.is_null() {
                return None;
            }
            // Prefer the bundle-id (stable, reverse-DNS). Fall back to the
            // localized name for the rare unbundled process.
            let bid: id = msg_send![app, bundleIdentifier];
            if let Some(s) = nsstring_to_string(bid) {
                if !s.is_empty() {
                    return Some(s);
                }
            }
            let name: id = msg_send![app, localizedName];
            nsstring_to_string(name).filter(|s| !s.is_empty())
        }
    }

    pub fn icon_data_url(app_key: &str) -> Option<String> {
        unsafe {
            let ws_cls = Class::get("NSWorkspace")?;
            let ws: id = msg_send![ws_cls, sharedWorkspace];
            if ws.is_null() {
                return None;
            }

            // bundle-id → app bundle path → icon. If the key isn't a known
            // bundle-id (e.g. a bare path slipped through), fall back to reading
            // the icon for the key treated as a filesystem path.
            let key_ns = string_to_nsstring(app_key)?;
            let url: id = msg_send![ws, URLForApplicationWithBundleIdentifier: key_ns];
            let icon: id = if !url.is_null() {
                let path: id = msg_send![url, path];
                match nsstring_to_string(path).and_then(|p| string_to_nsstring(&p)) {
                    Some(pns) => msg_send![ws, iconForFile: pns],
                    None => return None,
                }
            } else {
                msg_send![ws, iconForFile: key_ns]
            };
            if icon.is_null() {
                return None;
            }

            let png = nsimage_to_png(icon)?;
            Some(format!(
                "data:image/png;base64,{}",
                super::base64_encode(&png)
            ))
        }
    }

    /// Pick the representation nearest ICON_SIDE and PNG-encode just that one
    /// (not every rep up to 1024px — that's what makes a naive TIFF dump huge).
    unsafe fn nsimage_to_png(icon: id) -> Option<Vec<u8>> {
        let mut rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: ICON_SIDE,
                height: ICON_SIDE,
            },
        };
        // CGImageForProposedRect:context:hints: returns the CGImage of the rep
        // that best matches the proposed rect — a read, no drawing context, so
        // it's safe off the main thread (unlike lockFocus).
        let cg: *mut std::ffi::c_void = msg_send![
            icon,
            CGImageForProposedRect: &mut rect as *mut CGRect
            context: NIL
            hints: NIL
        ];
        if cg.is_null() {
            return None;
        }
        let rep_cls = Class::get("NSBitmapImageRep")?;
        let rep: id = msg_send![rep_cls, alloc];
        let rep: id = msg_send![rep, initWithCGImage: cg];
        if rep.is_null() {
            return None;
        }
        let dict_cls = Class::get("NSDictionary")?;
        let empty: id = msg_send![dict_cls, dictionary];
        let data: id = msg_send![
            rep,
            representationUsingType: NS_PNG_FILE_TYPE
            properties: empty
        ];
        if data.is_null() {
            return None;
        }
        let len: usize = msg_send![data, length];
        let bytes: *const u8 = msg_send![data, bytes];
        if bytes.is_null() || len == 0 {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(bytes, len) }.to_vec())
    }

    unsafe fn nsstring_to_string(s: id) -> Option<String> {
        if s.is_null() {
            return None;
        }
        let utf8: *const i8 = msg_send![s, UTF8String];
        if utf8.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(utf8) }
            .to_str()
            .ok()
            .map(|s| s.to_owned())
    }

    unsafe fn string_to_nsstring(s: &str) -> Option<id> {
        let cls = Class::get("NSString")?;
        let cstr = CString::new(s).ok()?;
        let ns: id = msg_send![cls, stringWithUTF8String: cstr.as_ptr()];
        if ns.is_null() { None } else { Some(ns) }
    }

    /// bundle-id → the app's on-disk bundle path (or the key itself if it's
    /// already a path that exists).
    unsafe fn app_bundle_path(app_key: &str) -> Option<String> {
        let ws_cls = Class::get("NSWorkspace")?;
        let ws: id = msg_send![ws_cls, sharedWorkspace];
        if ws.is_null() {
            return None;
        }
        let key_ns = unsafe { string_to_nsstring(app_key) }?;
        let url: id = msg_send![ws, URLForApplicationWithBundleIdentifier: key_ns];
        if !url.is_null() {
            let path: id = msg_send![url, path];
            return unsafe { nsstring_to_string(path) };
        }
        if Path::new(app_key).exists() {
            Some(app_key.to_string())
        } else {
            None
        }
    }

    /// Friendly, localized app name (e.g. "Google Chrome") via NSFileManager.
    pub fn display_name(app_key: &str) -> Option<String> {
        unsafe {
            let path = app_bundle_path(app_key)?;
            let fm_cls = Class::get("NSFileManager")?;
            let fm: id = msg_send![fm_cls, defaultManager];
            let path_ns = string_to_nsstring(&path)?;
            let name: id = msg_send![fm, displayNameAtPath: path_ns];
            let s = nsstring_to_string(name)?;
            let s = s.strip_suffix(".app").unwrap_or(&s).to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    }

    /// App Store category ("what it does") from the Spotlight index —
    /// `kMDItemAppStoreCategory`, e.g. "Developer Tools", "Productivity".
    pub fn category(app_key: &str) -> Option<String> {
        unsafe {
            let path = app_bundle_path(app_key)?;
            let url = CFURL::from_path(Path::new(&path), true)?;
            let item = MDItemCreateWithURL(
                std::ptr::null(),
                url.as_concrete_TypeRef() as *const std::ffi::c_void,
            );
            if item.is_null() {
                return None;
            }
            let attr = CFString::new("kMDItemAppStoreCategory");
            let val =
                MDItemCopyAttribute(item, attr.as_concrete_TypeRef() as *const std::ffi::c_void);
            CFRelease(item);
            if val.is_null() {
                return None;
            }
            // kMDItemAppStoreCategory is always a CFString; +1 from Copy → own it.
            let s = CFString::wrap_under_create_rule(val as CFStringRef).to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    }
}

// ── Windows ─────────────────────────────────────────────────────────────────
// Real implementation per the Win10/11 (2023–2026) research. Cannot be compiled
// or run from the macOS dev box — VERIFY on a Windows build. See the memory note
// `app-identity-service` for the full recipe (ApplicationFrameHost UWP unwrap,
// SHIL_JUMBO alpha fix) that the follow-up pass should add.
#[cfg(target_os = "windows")]
mod imp {
    use std::path::Path;

    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::core::PWSTR;

    pub fn app_key_for_pid(pid: i32) -> Option<String> {
        if pid <= 0 {
            return None;
        }
        unsafe {
            // PROCESS_QUERY_LIMITED_INFORMATION is the minimal right that works
            // for non-elevated callers across integrity levels (see research).
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid as u32).ok()?;
            let mut buf = [0u16; 32768];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(
                handle,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            );
            let _ = CloseHandle(handle);
            ok.ok()?;
            let s = String::from_utf16_lossy(&buf[..len as usize]);
            if s.is_empty() { None } else { Some(s) }
        }
    }

    pub fn icon_data_url(app_key: &str) -> Option<String> {
        // `app_key` is the exe path. `windows-icons` does the HICON→PNG dance
        // (GetIconInfo + GetDIBits + alpha reconstruction) and returns base64.
        // TODO(win): upgrade to SHIL_JUMBO 256px + UWP AppInfo::GetLogo branch.
        let b64 =
            std::panic::catch_unwind(|| windows_icons::get_icon_base64_by_path(app_key)).ok()?;
        if b64.is_empty() {
            return None;
        }
        Some(format!("data:image/png;base64,{b64}"))
    }

    pub fn display_name(app_key: &str) -> Option<String> {
        let name = Path::new(app_key)
            .file_stem()
            .and_then(|s| s.to_str())
            .or_else(|| Path::new(app_key).file_name().and_then(|s| s.to_str()))
            .unwrap_or(app_key)
            .trim();
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    }

    pub fn category(_app_key: &str) -> Option<String> {
        None
    }
}

// ── Other platforms ─────────────────────────────────────────────────────────
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    pub fn app_key_for_pid(_pid: i32) -> Option<String> {
        None
    }
    pub fn icon_data_url(_app_key: &str) -> Option<String> {
        None
    }
    pub fn display_name(_app_key: &str) -> Option<String> {
        None
    }
    pub fn category(_app_key: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_encodes_binary_png_header() {
        // \x89PNG\r\n\x1a\n
        let png_magic = [0x89u8, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        assert_eq!(base64_encode(&png_magic), "iVBORw0KGgo=");
    }
}
