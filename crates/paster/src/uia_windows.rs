//! UI Automation client for reading the user's current text selection.
//!
//! This is the Windows analog to macOS's `AXUIElementCopyAttributeValue(...,
//! kAXSelectedTextAttribute, ...)` path: a read-only OS API call that returns
//! "what is currently selected in the focused control" without synthesizing
//! any keystrokes, touching the clipboard, or racing with modifier-key state.
//!
//! Works in: Notepad, WordPad, Microsoft Office, Outlook, Visual Studio, .NET
//! and WPF apps, and most native Win32 controls.
//!
//! Does NOT work in: Chromium-based browsers (Chrome, Edge, Brave, Vivaldi)
//! and Electron apps (Slack, Discord, VS Code, GitHub Desktop) — those gate
//! their UIA TextPattern provider behind `--enable-features=UiaProvider`
//! which almost no end user has set. For those targets, `imp_windows`
//! transparently falls back to a Ctrl+C-and-read-clipboard path.
//!
//! Threading: `UIAutomation::new()` calls `CoInitializeEx(NULL,
//! COINIT_MULTITHREADED)` internally per-thread, so this is safe to call
//! from the `std::thread::spawn`'d worker that the keyboard hook dispatches
//! the shortcut callback onto.

use uiautomation::UIAutomation;
use uiautomation::patterns::UITextPattern;

/// Read the currently selected text from the focused control via UI Automation.
///
/// Returns `Some(text)` only when:
///   - There is a focused element (something has keyboard focus).
///   - That element exposes the TextPattern (most native controls do).
///   - `GetSelection` returns at least one range with non-empty text.
///
/// Returns `None` in every other case, which is the signal the caller uses
/// to fall back to the Ctrl+C/clipboard path. A `None` is silent on purpose
/// — the caller logs the dispatch outcome.
pub fn read_selected_text() -> Option<String> {
    // Two-step construction: try the COM-initializing path first (the
    // normal case for fresh worker threads spawned from the keyboard hook),
    // then fall back to the no-init path (the Tauri runtime threads that
    // already have COM apartment set, where `new()` would fail with
    // "Cannot change thread mode after it is set").
    let automation = match UIAutomation::new() {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(
                "[selection-uia] UIAutomation::new failed: {e} — retrying with new_direct()"
            );
            match UIAutomation::new_direct() {
                Ok(a) => a,
                Err(e2) => {
                    tracing::debug!("[selection-uia] UIAutomation::new_direct also failed: {e2}");
                    return None;
                }
            }
        }
    };

    let focused = match automation.get_focused_element() {
        Ok(el) => el,
        Err(e) => {
            tracing::debug!("[selection-uia] get_focused_element failed: {e}");
            return None;
        }
    };

    let text_pattern: UITextPattern = match focused.get_pattern::<UITextPattern>() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("[selection-uia] focused element doesn't expose TextPattern: {e}");
            return None;
        }
    };

    let ranges = match text_pattern.get_selection() {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("[selection-uia] get_selection failed: {e}");
            return None;
        }
    };

    if ranges.is_empty() {
        tracing::debug!("[selection-uia] no selection ranges returned");
        return None;
    }

    // Concatenate the text of each selected range. Most controls return a
    // single range; some grids / multi-selection controls return several.
    // Pass -1 for max_length to fetch the whole range (Win32 convention).
    let mut buf = String::new();
    for range in &ranges {
        match range.get_text(-1) {
            Ok(text) => buf.push_str(&text),
            Err(e) => {
                tracing::debug!("[selection-uia] range.get_text failed: {e}");
            }
        }
    }

    // GetSelection returns a "degenerate" (empty) range at the cursor when
    // nothing is selected. Treat empty/whitespace-only result as "no
    // selection" so the caller falls back to Ctrl+C — which might actually
    // find something the user thinks is selected (e.g., a non-focused
    // sibling control that retains selection visually).
    if buf.trim().is_empty() {
        tracing::debug!(
            "[selection-uia] returned empty/whitespace text — likely degenerate cursor range"
        );
        return None;
    }

    tracing::info!("[selection-uia] read {} chars via UI Automation", buf.len());
    Some(buf)
}
