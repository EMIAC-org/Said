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

/// True if `app_key` (a macOS bundle-id) is a browser we know how to script.
pub fn is_known_browser(app_key: &str) -> bool {
    browser_app_name(app_key).is_some()
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

/// Resolve the active tab's site for a browser bundle-id. `None` if it's not a
/// known browser, is incognito, has no scriptable URL, or Automation isn't
/// granted yet.
pub fn active_site(app_key: &str) -> Option<BrowserSite> {
    let (app, engine) = browser_app_name(app_key)?;
    imp::active_site(app, engine)
}

/// Send a benign Apple Event to `app_key` to trigger the per-browser Automation
/// consent prompt now (used by the "Enable" affordances), rather than mid-
/// dictation. Returns true if the event went through (already granted).
pub fn trigger_automation_prompt(app_key: &str) -> bool {
    match browser_app_name(app_key) {
        Some((app, _)) => imp::trigger_prompt(app),
        None => false,
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

    pub fn trigger_prompt(app: &str) -> bool {
        // Any Apple Event to the target fires the consent prompt; `get name` is
        // harmless and does not launch or alter the browser.
        run_osascript(&format!("tell application \"{app}\" to get name")).is_some()
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

// ── Non-macOS (Windows UIA is a follow-up) ──────────────────────────────────
#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{BrowserSite, Engine};

    pub fn active_site(_app: &str, _engine: Engine) -> Option<BrowserSite> {
        None
    }
    pub fn trigger_prompt(_app: &str) -> bool {
        false
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
