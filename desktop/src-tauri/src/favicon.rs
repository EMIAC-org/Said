//! Favicon fetching for the Insights "Sites you dictate in" section.
//!
//! PRIVACY: fetches the favicon DIRECTLY from the site
//! (`https://<host>/favicon.ico`), exactly like the user's own browser — never a
//! third-party favicon service (Google/DuckDuckGo), which would leak every
//! visited domain. Cached in-process. Returns a `data:` URL the webview renders
//! (ICO/PNG/SVG all render in an <img>); `None` on failure, so the frontend
//! draws a letter-tile fallback.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

fn cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve a host to a favicon `data:` URL, cached in-process (misses cached too,
/// so a site with no icon isn't re-fetched every render).
pub async fn favicon_data_url(host: &str) -> Option<String> {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    if let Ok(guard) = cache().lock() {
        if let Some(hit) = guard.get(&host) {
            return hit.clone();
        }
    }
    let result = fetch(&host).await;
    if let Ok(mut guard) = cache().lock() {
        guard.insert(host, result.clone());
    }
    result
}

async fn fetch(host: &str) -> Option<String> {
    let url = format!("https://{host}/favicon.ico");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent("AirNote")
        .build()
        .ok()?;
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    // Guard against empty responses and oversized junk.
    if bytes.len() < 4 || bytes.len() > 512 * 1024 {
        return None;
    }
    let mime = sniff_mime(&bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        crate::app_identity::base64_encode(&bytes)
    ))
}

/// Identify a renderable image by magic bytes; rejects HTML 404 pages (which
/// `/favicon.ico` often returns) by returning `None` for anything unrecognised.
fn sniff_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some("image/png");
    }
    if b.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some("image/x-icon");
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if b.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if b.starts_with(b"RIFF") {
        return Some("image/webp");
    }
    // SVG (possibly with an XML prolog) — scan a small window.
    let head = &b[..b.len().min(128)];
    if head.windows(4).any(|w| w.eq_ignore_ascii_case(b"<svg")) {
        return Some("image/svg+xml");
    }
    None
}

#[cfg(test)]
mod tests {
    use super::sniff_mime;

    #[test]
    fn sniffs_known_types_and_rejects_html() {
        assert_eq!(
            sniff_mime(&[0x89, b'P', b'N', b'G', 1, 2]),
            Some("image/png")
        );
        assert_eq!(
            sniff_mime(&[0x00, 0x00, 0x01, 0x00, 1]),
            Some("image/x-icon")
        );
        assert_eq!(sniff_mime(b"<svg xmlns=..."), Some("image/svg+xml"));
        assert_eq!(sniff_mime(b"<!DOCTYPE html><html>404"), None);
        assert_eq!(sniff_mime(b"nope"), None);
    }
}
