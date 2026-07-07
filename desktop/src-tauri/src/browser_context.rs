//! Browser tab context — site-level dictation mapping.
//!
//! When a dictation is pasted into a browser, resolve the *active tab's* site so
//! the knowledge base can tell Gmail from Twitter inside the same Chrome. macOS
//! only for now (AppleScript / Apple Events); Windows (UI Automation) is a
//! follow-up (see the `browser-tab-context` design note).
//!
//! PRIVACY — non-negotiable:
//!   - We read the URL but return ONLY the host (`mail.google.com`). Scheme,
//!     path and query are dropped in `host_from_url` and never surface — so a
//!     `?q=<search terms>` or `/document/d/<id>` can't leak.
//!   - Chrome-family **incognito** windows are skipped (`mode == "incognito"`).
//!   - This module is inert until the caller (which gates on the opt-in pref)
//!     asks. Nothing here is stored or transmitted — the caller decides.
//!
//! Reaching the browser sends an Apple Event, which triggers the per-target
//! macOS Automation consent prompt the first time. Until granted, `osascript`
//! fails and we return `None` (no context, no error surfaced to the user).

#[derive(Clone, Debug, serde::Serialize)]
pub struct BrowserSite {
    /// Host only, lowercased (e.g. "mail.google.com"). Never a full URL.
    pub host: String,
    /// Tab title, best-effort (may be empty). For display; callers that persist
    /// should treat it as sensitive (titles can carry doc names).
    pub title: String,
}

/// True if `app_key` is a browser we know how to read. `app_key` is a macOS
/// bundle-id (`com.google.Chrome`) or, on Windows, the exe path
/// (`…\chrome.exe`) — so detection is platform-specific.
pub fn is_known_browser(app_key: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        browser_app_name(app_key).is_some()
    }
    #[cfg(not(target_os = "macos"))]
    {
        imp::is_known_browser(app_key)
    }
}

/// macOS Automation (Apple Events) consent state for one target browser.
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AutomationStatus {
    /// Consent granted — we can read the active tab.
    Granted,
    /// User explicitly denied in the prompt / System Settings.
    Denied,
    /// Never asked yet, or the browser isn't running so we can't tell.
    Unknown,
}

/// One known browser, whether it's running, and its live Automation status.
/// Powers the Settings per-browser grant list.
#[derive(Clone, Debug, serde::Serialize)]
pub struct BrowserAutomation {
    pub app_key: String,
    pub name: String,
    pub running: bool,
    pub status: AutomationStatus,
}

/// Preflight the Automation consent for `app_key` WITHOUT prompting — used by the
/// UI to show granted/denied/not-asked. `Unknown` for non-browsers.
pub fn automation_status(app_key: &str) -> AutomationStatus {
    if is_known_browser(app_key) {
        imp::automation_status(app_key)
    } else {
        AutomationStatus::Unknown
    }
}

/// Every known browser currently running, with its live Automation status, so
/// Settings can show real per-browser state and a working "Grant" button.
pub fn running_browser_automation() -> Vec<BrowserAutomation> {
    imp::running_bundle_ids()
        .into_iter()
        .filter_map(|b| browser_app_name(&b).map(|(name, _)| (b, name)))
        .map(|(app_key, name)| {
            let status = imp::automation_status(&app_key);
            BrowserAutomation {
                app_key,
                name: name.to_string(),
                running: true,
                status,
            }
        })
        .collect()
}

/// bundle-id → the AppleScript application name. `None` for non-browsers.
fn browser_app_name(app_key: &str) -> Option<(&'static str, Engine)> {
    let e = |name, engine| Some((name, engine));
    match app_key {
        "com.google.Chrome" => e("Google Chrome", Engine::Chromium),
        "com.google.Chrome.canary" => e("Google Chrome Canary", Engine::Chromium),
        "com.brave.Browser" => e("Brave Browser", Engine::Chromium),
        "com.brave.Browser.beta" => e("Brave Browser Beta", Engine::Chromium),
        "com.microsoft.edgemac" => e("Microsoft Edge", Engine::Chromium),
        "com.vivaldi.Vivaldi" => e("Vivaldi", Engine::Chromium),
        "org.chromium.Chromium" => e("Chromium", Engine::Chromium),
        "company.thebrowser.Browser" => e("Arc", Engine::Chromium),
        "company.thebrowser.dia" => e("Dia", Engine::Chromium),
        "com.browseros.BrowserOS" => e("BrowserOS", Engine::Chromium),
        "at.studio.AsideBrowser" => e("Aside", Engine::Chromium),
        "com.operasoftware.Opera" => e("Opera", Engine::Chromium),
        "com.operasoftware.OperaGX" => e("Opera GX", Engine::Chromium),
        "com.apple.Safari" => e("Safari", Engine::Safari),
        // Unknown browsers fall through to None → app-level context only. Adding
        // a wrong app name here is safe: osascript just errors and we return None.
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum Engine {
    Chromium,
    Safari,
}

/// Resolve the active tab's site for a browser `app_key`. `None` if it's not a
/// known browser, is incognito, has no readable URL, or (macOS) Automation isn't
/// granted yet.
pub fn active_site(app_key: &str) -> Option<BrowserSite> {
    #[cfg(target_os = "macos")]
    {
        let (app, engine) = browser_app_name(app_key)?;
        imp::active_site(app, engine)
    }
    #[cfg(not(target_os = "macos"))]
    {
        imp::active_site(app_key)
    }
}

/// Trigger the per-browser Automation consent prompt now (used by the "Enable"
/// affordances), rather than mid-dictation. Uses the native
/// `AEDeterminePermissionToAutomateTarget` so the dialog is attributed to AirNote
/// itself. Returns true if consent is already granted.
pub fn trigger_automation_prompt(app_key: &str) -> bool {
    if is_known_browser(app_key) {
        imp::trigger_prompt(app_key)
    } else {
        false
    }
}

/// Prompt the macOS Automation consent for every currently-running known
/// browser, so the user grants it upfront (from an "Enable" button) instead of
/// mid-dictation. Returns the display names of the browsers prompted.
pub fn request_automation_upfront() -> Vec<String> {
    imp::running_bundle_ids()
        .into_iter()
        .filter(|b| is_known_browser(b))
        .filter_map(|b| {
            trigger_automation_prompt(&b);
            browser_app_name(&b).map(|(name, _)| name.to_string())
        })
        .collect()
}

/// Extract the bare host from a (possibly scheme-less) URL, dropping scheme,
/// userinfo, port, path, query and fragment. This is the privacy chokepoint:
/// the path/query never leave this function.
fn host_from_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s == "missing value" {
        return None;
    }
    // Only web schemes carry a site; skip chrome://, about:, file:, data:, etc.
    let after_scheme = match s.split_once("://") {
        Some((scheme, rest)) => {
            let sc = scheme.to_ascii_lowercase();
            if sc != "http" && sc != "https" {
                return None;
            }
            rest
        }
        None => s, // scheme-less (Windows omnibox style) — assume web
    };
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    // strip userinfo then port
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let host = host.split(':').next().unwrap_or(host);
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host)
}

// ── macOS ───────────────────────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod imp {
    use super::{BrowserSite, Engine, host_from_url};

    const DELIM: &str = "|~|";

    pub fn active_site(app: &str, engine: Engine) -> Option<BrowserSite> {
        let script = match engine {
            // `try` makes `mode` optional so browsers without it (Arc) still work.
            Engine::Chromium => format!(
                "tell application \"{app}\"\n\
                 set m to \"normal\"\n\
                 try\n\
                 set m to (mode of front window) as text\n\
                 end try\n\
                 set u to URL of active tab of front window\n\
                 set t to title of active tab of front window\n\
                 return m & \"{DELIM}\" & u & \"{DELIM}\" & t\n\
                 end tell"
            ),
            Engine::Safari => format!(
                "tell application \"{app}\"\n\
                 set u to URL of front document\n\
                 set t to name of front document\n\
                 return \"normal\" & \"{DELIM}\" & u & \"{DELIM}\" & t\n\
                 end tell"
            ),
        };
        let out = run_osascript(&script)?;
        let mut parts = out.splitn(3, DELIM);
        let mode = parts.next().unwrap_or("");
        let url = parts.next().unwrap_or("");
        let title = parts.next().unwrap_or("");
        if mode.eq_ignore_ascii_case("incognito") {
            return None; // respect private browsing
        }
        let host = host_from_url(url)?;
        Some(BrowserSite {
            host,
            title: title.trim().to_string(),
        })
    }

    /// Fire the Automation consent dialog for a target browser (by bundle-id),
    /// attributed to AirNote. true if consent is already granted.
    pub fn trigger_prompt(bundle_id: &str) -> bool {
        ae::prompt(bundle_id)
    }

    /// Live Automation consent state for a target browser (by bundle-id), without
    /// prompting.
    pub fn automation_status(bundle_id: &str) -> super::AutomationStatus {
        match ae::status(bundle_id) {
            ae::Status::Granted => super::AutomationStatus::Granted,
            ae::Status::Denied => super::AutomationStatus::Denied,
            ae::Status::Unknown => super::AutomationStatus::Unknown,
        }
    }

    /// Native Apple Events permission API. `AEDeterminePermissionToAutomateTarget`
    /// is Apple's canonical way to both preflight (ask=false → real status) and
    /// request (ask=true → consent dialog) Automation permission for a target app,
    /// sent from *this* process so TCC attributes it to AirNote — unlike shelling
    /// out to `osascript`, which was our earlier, unreliable trigger.
    mod ae {
        use std::os::raw::c_void;

        type OSStatus = i32;
        type OSType = u32;
        type Boolean = u8;

        #[repr(C)]
        struct AEDesc {
            descriptor_type: OSType,
            data_handle: *mut c_void,
        }

        const fn four_cc(s: &[u8; 4]) -> OSType {
            ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
        }

        // Address the target by bundle-id; wildcard event class/id checks overall
        // Automation permission (Apple's documented pattern).
        const TYPE_APPLICATION_BUNDLE_ID: OSType = four_cc(b"bund");
        const TYPE_WILDCARD: OSType = four_cc(b"****");

        const NO_ERR: OSStatus = 0;
        const ERR_AE_EVENT_NOT_PERMITTED: OSStatus = -1743;
        const ERR_AE_EVENT_WOULD_REQUIRE_USER_CONSENT: OSStatus = -1744;
        const PROC_NOT_FOUND: OSStatus = -600;

        #[link(name = "CoreServices", kind = "framework")]
        unsafe extern "C" {
            fn AECreateDesc(
                type_code: OSType,
                data_ptr: *const c_void,
                data_size: isize,
                result: *mut AEDesc,
            ) -> OSStatus;
            fn AEDisposeDesc(desc: *mut AEDesc) -> OSStatus;
            fn AEDeterminePermissionToAutomateTarget(
                target: *const AEDesc,
                the_ae_event_class: OSType,
                the_ae_event_id: OSType,
                ask_user_if_needed: Boolean,
            ) -> OSStatus;
        }

        pub enum Status {
            Granted,
            Denied,
            Unknown,
        }

        fn determine(bundle_id: &str, ask: bool) -> OSStatus {
            let bytes = bundle_id.as_bytes();
            let mut desc = AEDesc {
                descriptor_type: 0,
                data_handle: std::ptr::null_mut(),
            };
            unsafe {
                let err = AECreateDesc(
                    TYPE_APPLICATION_BUNDLE_ID,
                    bytes.as_ptr() as *const c_void,
                    bytes.len() as isize,
                    &mut desc,
                );
                if err != NO_ERR {
                    return err;
                }
                let status = AEDeterminePermissionToAutomateTarget(
                    &desc,
                    TYPE_WILDCARD,
                    TYPE_WILDCARD,
                    Boolean::from(ask),
                );
                AEDisposeDesc(&mut desc);
                status
            }
        }

        /// Preflight, no prompt — real granted/denied/not-asked state for the UI.
        pub fn status(bundle_id: &str) -> Status {
            match determine(bundle_id, false) {
                NO_ERR => Status::Granted,
                ERR_AE_EVENT_NOT_PERMITTED => Status::Denied,
                // -1744 never asked; -600 browser not running → can't tell yet.
                ERR_AE_EVENT_WOULD_REQUIRE_USER_CONSENT | PROC_NOT_FOUND => Status::Unknown,
                _ => Status::Unknown,
            }
        }

        /// Fire the consent dialog. true if already granted (no dialog shown).
        pub fn prompt(bundle_id: &str) -> bool {
            determine(bundle_id, true) == NO_ERR
        }
    }

    /// Bundle-ids of all currently-running apps, via NSWorkspace.
    pub fn running_bundle_ids() -> Vec<String> {
        use std::ffi::CStr;

        use objc::runtime::{Class, Object};
        use objc::{msg_send, sel, sel_impl};

        let mut out = Vec::new();
        unsafe {
            let cls = match Class::get("NSWorkspace") {
                Some(c) => c,
                None => return out,
            };
            let ws: *mut Object = msg_send![cls, sharedWorkspace];
            if ws.is_null() {
                return out;
            }
            let apps: *mut Object = msg_send![ws, runningApplications];
            if apps.is_null() {
                return out;
            }
            let count: usize = msg_send![apps, count];
            for i in 0..count {
                let app: *mut Object = msg_send![apps, objectAtIndex: i];
                if app.is_null() {
                    continue;
                }
                let bid: *mut Object = msg_send![app, bundleIdentifier];
                if bid.is_null() {
                    continue;
                }
                let utf8: *const i8 = msg_send![bid, UTF8String];
                if utf8.is_null() {
                    continue;
                }
                if let Ok(s) = CStr::from_ptr(utf8).to_str() {
                    out.push(s.to_owned());
                }
            }
        }
        out
    }

    fn run_osascript(script: &str) -> Option<String> {
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .ok()?;
        if !output.status.success() {
            // -1743 denied / -600 not running / -1728 no window → no context.
            return None;
        }
        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }
}

// ── Windows (UI Automation — no OS permission needed) ────────────────────────
//
// Unlike macOS, Windows has NO Automation consent gate: a same-integrity process
// can read another window's UI tree freely. So there is nothing to prompt for and
// nothing to "grant" — `automation_status` is always Granted and `trigger_prompt`
// is a no-op success. We read the foreground browser's omnibox URL via UIA.
//
// COMPILE-VERIFIED for x86_64-pc-windows-msvc via an isolated probe crate (the
// desktop crate itself can't cross-check on macOS — `ring`'s C build blocks it —
// so the uiautomation v0.25 call sites + the HWND→isize→Handle bridge were built
// standalone against windows 0.58). RUNTIME still to confirm on a real Windows
// box: the "Address and search bar" omnibox name and LegacyIAccessibleValue read.
#[cfg(target_os = "windows")]
mod imp {
    use uiautomation::UIAutomation;
    use uiautomation::controls::ControlType;
    use uiautomation::types::{Handle, UIProperty};
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    use super::{BrowserSite, host_from_url};

    /// `app_key` is the exe path from app_identity (`…\chrome.exe`). Match on the
    /// lowercased file name across the Chromium family + Firefox.
    pub fn is_known_browser(app_key: &str) -> bool {
        matches!(
            exe_name(app_key).as_deref(),
            Some(
                "chrome.exe"
                    | "msedge.exe"
                    | "brave.exe"
                    | "vivaldi.exe"
                    | "opera.exe"
                    | "opera_gx.exe"
                    | "arc.exe"
                    | "chromium.exe"
                    | "browser.exe"
                    | "firefox.exe"
            )
        )
    }

    fn exe_name(path: &str) -> Option<String> {
        path.rsplit(['\\', '/']).next().map(str::to_ascii_lowercase)
    }

    /// Read the foreground browser's omnibox URL via UI Automation, host only.
    pub fn active_site(_app_key: &str) -> Option<BrowserSite> {
        let hwnd = unsafe { GetForegroundWindow() };
        // uiautomation bundles its own `windows` (0.62) whose HWND type differs
        // from ours (0.58) — bridge through `isize` via `Handle: From<isize>`.
        let raw = hwnd.0 as isize;
        if raw == 0 {
            return None;
        }
        let automation = UIAutomation::new().ok()?; // inits COM internally
        let root = automation.element_from_handle(Handle::from(raw)).ok()?;
        // Window title ("<page> - <Browser>"), best-effort; grabbed before `root`
        // is moved into the matcher.
        let title = root.get_name().unwrap_or_default();
        // Chromium/Edge omnibox: an Edit named "Address and search bar". (Firefox
        // exposes a different tree — its URL isn't found here yet; returns None.)
        let edit = automation
            .create_matcher()
            .from(root)
            .control_type(ControlType::Edit)
            .name("Address and search bar")
            .timeout(300)
            .find_first()
            .ok()?;
        // LegacyIAccessible value is more reliable than ValueValue for the omnibox.
        let raw_url = edit
            .get_property_value(UIProperty::LegacyIAccessibleValue)
            .ok()?
            .get_string()
            .ok()?;
        let host = host_from_url(&raw_url)?;
        Some(BrowserSite { host, title })
    }

    pub fn trigger_prompt(_bundle_id: &str) -> bool {
        true // Windows UIA needs no consent — nothing to prompt.
    }
    pub fn automation_status(_bundle_id: &str) -> super::AutomationStatus {
        super::AutomationStatus::Granted // no permission gate on Windows
    }
    pub fn running_bundle_ids() -> Vec<String> {
        Vec::new() // per-target consent is macOS-only; unused on Windows
    }
}

// ── Other platforms ─────────────────────────────────────────────────────────
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use super::BrowserSite;

    pub fn is_known_browser(_app_key: &str) -> bool {
        false
    }
    pub fn active_site(_app_key: &str) -> Option<BrowserSite> {
        None
    }
    pub fn trigger_prompt(_bundle_id: &str) -> bool {
        false
    }
    pub fn automation_status(_bundle_id: &str) -> super::AutomationStatus {
        super::AutomationStatus::Unknown
    }
    pub fn running_bundle_ids() -> Vec<String> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_strips_path_query_scheme() {
        assert_eq!(
            host_from_url("https://mail.google.com/mail/u/0/#inbox?q=secret+terms"),
            Some("mail.google.com".to_string())
        );
        assert_eq!(
            host_from_url("http://User@Example.COM:8080/a/b?c=d"),
            Some("example.com".to_string())
        );
        // scheme-less (Windows omnibox style)
        assert_eq!(
            host_from_url("twitter.com/home"),
            Some("twitter.com".to_string())
        );
    }

    #[test]
    fn host_rejects_non_web_and_junk() {
        assert_eq!(host_from_url("chrome://settings"), None);
        assert_eq!(host_from_url("about:blank"), None);
        assert_eq!(host_from_url("file:///Users/me/x.html"), None);
        assert_eq!(host_from_url("missing value"), None);
        assert_eq!(host_from_url(""), None);
        assert_eq!(host_from_url("localhost"), None); // no dot → not a site
    }

    #[test]
    fn known_browsers_detected() {
        assert!(is_known_browser("com.google.Chrome"));
        assert!(is_known_browser("com.apple.Safari"));
        assert!(!is_known_browser("com.tinyspeck.slackmacgap"));
    }
}
