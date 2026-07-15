#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod app_identity; // resolve target_app pid → bundle-id/exe + app icon (App Identity Service)
mod asr; // adaptive, process-isolated on-device dictation ASR (GPU worker + CPU safety)
mod backend;
mod backend_guard;
mod browser_context; // opt-in: active browser tab → site host (AppleScript, macOS)
mod chaos; // env-gated fault injection for torture-testing the resilience paths
mod desktop;
mod developer_context;
mod diag; // lock-holder + breadcrumb instrumentation for stuck-state diagnostics
mod dictation_stt;
mod divo; // Ctrl hold-to-talk → Divo agent bridge (SSE proxy via control-plane)
mod echo_gate;
mod enterprise_oauth;
mod favicon; // direct /favicon.ico fetch + cache for the Insights "Sites" section
mod hud_manager;
mod meeting_engine;
mod nemotron; // optional experimental GGUF local-ASR provider; never used by Meetings
mod notch_sidecar;
mod whisper_dictation_stream; // crash-recovery PCM tap during on-device dictation (batch-only STT)
// mod meeting_audio; // Removed: meeting mode reuses the main pipeline
mod permissions;
mod recovery; // crash-safe dictation audio capture + relaunch recovery
mod speaker_suppression;
mod stt_policy;
mod telemetry;

use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tauri::{
    Emitter, Manager, State,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};
use tokio_util::sync::CancellationToken;

use backend::BackendEndpoint;
use desktop::DesktopApp;
use meeting_engine::{
    meeting_engine_add_user_tag, meeting_engine_chat, meeting_engine_clear_empty_meetings,
    meeting_engine_delete_digest, meeting_engine_delete_meeting_files, meeting_engine_digest_chat,
    meeting_engine_dismiss_ai_tag, meeting_engine_generate_digest,
    meeting_engine_generate_intelligence, meeting_engine_get_cached_artifacts,
    meeting_engine_get_cached_intelligence, meeting_engine_get_digest,
    meeting_engine_get_live_transcript, meeting_engine_get_manual_actions,
    meeting_engine_get_meeting_overviews, meeting_engine_get_notes,
    meeting_engine_get_processing_status, meeting_engine_get_status, meeting_engine_get_user_tags,
    meeting_engine_list_digests, meeting_engine_list_meetings, meeting_engine_new_local_meeting,
    meeting_engine_remove_user_tag, meeting_engine_retranscribe, meeting_engine_search_meetings,
    meeting_engine_set_manual_actions, meeting_engine_set_meeting_favorite,
    meeting_engine_set_meeting_hidden, meeting_engine_set_meeting_lark_doc,
    meeting_engine_set_meeting_title, meeting_engine_set_notes, meeting_engine_start_session,
    meeting_engine_stop_session, meeting_engine_take_discarded_meeting_ids,
    meeting_engine_toggle_mute,
};
use said_core::{AppSnapshot, ProcessSummary};
use said_paster as paster;

const DEBUG_LOG_MAX_BYTES: u64 = 240_000;
const STREAM_RESET_SENTINEL: &str = "\u{1F}__RESET__\u{1F}";
const MEETING_SPEECH_LEVEL: f32 = 0.025;
const MEETING_SILENCE_LEVEL: f32 = 0.012;
const MEETING_PAUSE_MS: u64 = 900;
const MEETING_MIN_CHUNK_MS: u64 = 700;
const MEETING_MAX_CHUNK_MS: u64 = 30_000;
const AUTOSTART_ARG: &str = "--airnote-autostart";
const SWIFT_FINAL_HANDOFF_WAIT_MS: u64 = 8_500;
const DICTATION_NO_SPEECH_PEAK: f64 = 0.0035;
const DICTATION_NO_SPEECH_RMS: f64 = 0.0008;
const VOICE_ERROR_ALREADY_EMITTED_PREFIX: &str = "__airnote_voice_error_already_emitted__:";
fn is_short_recording_cancel(err: &str) -> bool {
    err == desktop::RECORDING_TOO_SHORT_ERROR
}

// ── Main-thread panic seatbelt ────────────────────────────────────────────────
//
// A Rust panic that unwinds across the Objective-C → Rust boundary (any AppKit
// callback: tray menu, window events, `run_on_main_thread` dispatches) aborts the
// whole process with SIGABRT. `guard_panics` runs such a callback inside
// `catch_unwind` so a panic is caught, logged, and reported — the app stays alive
// and only the offending UI event is dropped.
//
// The global panic hook (installed in `main`) kills the backend sidecar on a
// *fatal* panic so it doesn't leak. When we are inside a guarded section the app
// will survive, so the hook must NOT kill the backend — `GUARD_DEPTH` lets the
// hook tell the two apart. The hook runs on the panicking thread before unwinding,
// so this thread-local is observed correctly.
thread_local! {
    static GUARD_DEPTH: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// True when the current thread is executing inside `guard_panics`, i.e. a panic
/// here is recoverable and the backend must be left alone.
fn in_guarded_section() -> bool {
    GUARD_DEPTH.with(|d| d.get() > 0)
}

/// Run `f`, catching any panic so it cannot abort the process. Returns `None` if
/// `f` panicked (already logged + reported), `Some(value)` otherwise.
fn guard_panics<R>(label: &'static str, f: impl FnOnce() -> R) -> Option<R> {
    GUARD_DEPTH.with(|d| d.set(d.get() + 1));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    GUARD_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    match result {
        Ok(value) => Some(value),
        Err(_) => {
            tracing::error!(
                "[guard] recovered from panic in main-thread callback: {label} — app kept alive"
            );
            diag::breadcrumb(format!("guard:recovered:{label}"));
            said_core::reporter::report_event(
                "panic.recovered",
                said_core::reporter::Severity::Error,
                serde_json::json!({ "where": label, "trail": diag::breadcrumbs(20) }),
            );
            None
        }
    }
}

/// Dispatch `f` to the AppKit main thread wrapped in the panic seatbelt. Use this
/// instead of `run_on_main_thread` for any closure touching windows/panels/tray so
/// a panic inside it can never SIGABRT the app.
pub(crate) fn run_on_main_guarded(
    app: &tauri::AppHandle,
    label: &'static str,
    f: impl FnOnce() + Send + 'static,
) -> Result<(), tauri::Error> {
    app.run_on_main_thread(move || {
        guard_panics(label, f);
    })
}

#[cfg(target_os = "macos")]
fn exit_after_shutdown_cleanup(code: i32) -> ! {
    // whisper.cpp/ggml Metal has process-exit finalizers that can abort inside
    // ggml_metal_rsets_free after a clean app shutdown. We have already run the
    // app-owned cleanup before calling this, so skip C/C++ finalizers on macOS.
    unsafe { libc::_exit(code) }
}

#[cfg(not(target_os = "macos"))]
fn exit_after_shutdown_cleanup(code: i32) -> ! {
    std::process::exit(code)
}

#[cfg(target_os = "macos")]
fn configure_ggml_metal_shutdown_guard() {
    if std::env::var_os("GGML_METAL_NO_RESIDENCY").is_none() {
        // Must be set before whisper.cpp/ggml initializes Metal. Newer ggml
        // residency-set teardown can assert during libc finalizers on app quit.
        unsafe { std::env::set_var("GGML_METAL_NO_RESIDENCY", "1") };
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_ggml_metal_shutdown_guard() {}

fn record_hotkey_label(raw: &str) -> &'static str {
    // Platform-aware: the same pref maps to different physical keys. On Windows
    // `right_option` binds to Right Alt (VK_RMENU) and `fn` degrades to Caps Lock.
    #[cfg(target_os = "windows")]
    match raw {
        "right_option" => "Right Alt",
        "fn" => "Caps Lock",
        _ => "Caps Lock",
    }
    #[cfg(not(target_os = "windows"))]
    match raw {
        "right_option" => "Right Option",
        "fn" => "Fn",
        _ => "Caps Lock",
    }
}

fn emit_short_recording_error(app: &tauri::AppHandle) {
    let record_hotkey = app
        .try_state::<TrayCache>()
        .map(|cache| {
            cache
                .0
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .record_hotkey
                .clone()
        })
        .unwrap_or_else(|| "caps_lock".to_string());
    let message = format!("Hold {} to record", record_hotkey_label(&record_hotkey));
    let _ = app.emit(
        "voice-error",
        serde_json::json!({
            "message": message,
            "audio_id": null,
            "auto_hide_ms": 1800,
        }),
    );
}

// said-hotkey ships a Windows impl (WH_KEYBOARD_LL) alongside the macOS
// CGEventTap, and Linux falls back to no-op stubs. Either way it's safe to
// import unconditionally.
use said_hotkey as hotkey;

#[cfg(target_os = "macos")]
use tauri_nspanel::{
    CollectionBehavior, ManagerExt as PanelManagerExt, PanelBuilder, PanelLevel, StyleMask,
    tauri_panel,
};

#[cfg(target_os = "macos")]
tauri_panel! {
    panel!(StatusBarPanel {
        config: {
            can_become_key_window: false,
            can_become_main_window: false,
            is_floating_panel: true,
            hides_on_deactivate: false,
            works_when_modal: true
        }
    })

    // Like StatusBarPanel but able to become key so a click registers (the pill
    // restores the app on click). Still non-activating so it floats over — and
    // doesn't steal a Space from — a full-screen meeting app.
    panel!(MeetingPillPanel {
        config: {
            can_become_key_window: true,
            can_become_main_window: false,
            is_floating_panel: true,
            hides_on_deactivate: false,
            works_when_modal: true
        }
    })
}

pub(crate) fn emit_status_bar_resync(app: &tauri::AppHandle, reason: &str) {
    let state = app
        .try_state::<SharedApp>()
        .and_then(|shared| shared.0.lock().ok().map(|d| d.state.as_str().to_string()))
        .unwrap_or_else(|| "unknown".to_string());
    let _ = app.emit(
        "status-bar-resync",
        serde_json::json!({
            "reason": reason,
            "state": state,
        }),
    );
}

#[cfg(target_os = "macos")]
fn configure_main_window_macos(_win: &tauri::WebviewWindow) {
    // The dynamic acceptsFirstMouse subclassing path caused foreign Cocoa
    // exceptions to unwind through tao's event loop in dev builds.
}

#[cfg(not(target_os = "macos"))]
fn configure_main_window_macos(_win: &tauri::WebviewWindow) {}

const STATUS_BAR_WIDTH: f64 = 300.0;
const STATUS_BAR_HEIGHT: f64 = 142.0;
const STATUS_BAR_BOTTOM_OFFSET: f64 = 64.0;

/// Bottom-center anchor — size-independent so resize keeps the bar grounded.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct StatusBarAnchor {
    center_x: f64,
    bottom_y: f64,
}

/// Legacy top-left format from older builds.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
struct StatusBarPositionLegacy {
    x: f64,
    y: f64,
}

fn status_bar_position_path() -> std::path::PathBuf {
    said_core::paths::data_dir().join("status_bar_position.json")
}

fn load_status_bar_anchor() -> Option<StatusBarAnchor> {
    let text = std::fs::read_to_string(status_bar_position_path()).ok()?;
    if let Ok(anchor) = serde_json::from_str::<StatusBarAnchor>(&text) {
        return Some(anchor);
    }
    let legacy = serde_json::from_str::<StatusBarPositionLegacy>(&text).ok()?;
    Some(StatusBarAnchor {
        center_x: legacy.x + STATUS_BAR_WIDTH / 2.0,
        bottom_y: legacy.y + STATUS_BAR_HEIGHT,
    })
}

fn save_status_bar_anchor(anchor: StatusBarAnchor) -> Result<(), String> {
    let path = status_bar_position_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create data dir: {e}"))?;
    }
    let text =
        serde_json::to_string_pretty(&anchor).map_err(|e| format!("serialize position: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write position: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("commit position: {e}"))?;
    Ok(())
}

fn clear_status_bar_position() -> Result<(), String> {
    let path = status_bar_position_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("remove position: {e}"))?;
    }
    Ok(())
}

fn status_bar_origin_from_anchor(anchor: StatusBarAnchor, width: f64, height: f64) -> (f64, f64) {
    (anchor.center_x - width / 2.0, anchor.bottom_y - height)
}

fn status_bar_target_origin(app: &tauri::AppHandle, width: f64, height: f64) -> (f64, f64) {
    if let Some(anchor) = load_status_bar_anchor() {
        return status_bar_origin_from_anchor(anchor, width, height);
    }
    status_bar_origin_for_cursor(app, width, height)
}

fn apply_status_bar_position(
    app: &tauri::AppHandle,
    win: &tauri::WebviewWindow,
) -> Result<(), String> {
    let scale = win.scale_factor().unwrap_or(1.0);
    let size = win.inner_size().unwrap_or(tauri::PhysicalSize::new(
        (STATUS_BAR_WIDTH * scale) as u32,
        (STATUS_BAR_HEIGHT * scale) as u32,
    ));
    let w = size.width as f64 / scale;
    let h = size.height as f64 / scale;
    let (x, y) = status_bar_target_origin(app, w, h);
    win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
        .map_err(|e| format!("set position failed: {e}"))
}

/// Keep the floating status bar visible at idle only when `SAID_STATUS_BAR_PIN=1`.
pub(crate) fn status_bar_pinned() -> bool {
    matches!(
        std::env::var("SAID_STATUS_BAR_PIN").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

/// Bottom-center origin for the status-bar window on the monitor that contains
/// the cursor (falls back to primary / first monitor).
fn status_bar_origin_for_cursor(app: &tauri::AppHandle, width: f64, height: f64) -> (f64, f64) {
    let origin_for_monitor = |m: &tauri::Monitor| -> (f64, f64) {
        let sf = m.scale_factor();
        let mx = m.position().x as f64 / sf;
        let my = m.position().y as f64 / sf;
        let sw = m.size().width as f64 / sf;
        let sh = m.size().height as f64 / sf;
        (
            mx + sw / 2.0 - width / 2.0,
            my + sh - height - STATUS_BAR_BOTTOM_OFFSET,
        )
    };

    let Ok(monitors) = app.available_monitors() else {
        return (560.0, 860.0);
    };
    let Some(primary) = monitors.first() else {
        return (560.0, 860.0);
    };

    let Ok(cursor) = app.cursor_position() else {
        return origin_for_monitor(primary);
    };

    let cx = cursor.x;
    let cy = cursor.y;
    let chosen = monitors.iter().find(|m| {
        let p = m.position();
        let s = m.size();
        let left = p.x as f64;
        let top = p.y as f64;
        cx >= left && cx < left + s.width as f64 && cy >= top && cy < top + s.height as f64
    });

    origin_for_monitor(chosen.unwrap_or(primary))
}

fn reposition_status_bar(app: &tauri::AppHandle, win: &tauri::WebviewWindow) {
    match apply_status_bar_position(app, win) {
        Ok(_) => tracing::debug!("[status-bar] repositioned"),
        Err(e) => tracing::warn!("[status-bar] reposition failed: {e}"),
    }
}

/// Must run on the AppKit main thread (never from the CGEventTap hotkey thread).
#[cfg(target_os = "macos")]
fn present_status_bar_macos_on_main(
    app: &tauri::AppHandle,
    win: &tauri::WebviewWindow,
    state: &str,
    resync: bool,
) {
    diag::breadcrumb(format!("status_bar:present:{state}:begin"));
    reposition_status_bar(app, win);
    diag::breadcrumb(format!("status_bar:present:{state}:repositioned"));
    hud_manager::configure_window(win);
    if let Err(e) = win.set_always_on_top(true) {
        tracing::warn!("[status-bar] set_always_on_top failed: {e}");
    }
    diag::breadcrumb(format!("status_bar:present:{state}:show"));
    match win.show() {
        Ok(_) => tracing::debug!("[status-bar] show ok for state={state}"),
        Err(e) => tracing::warn!("[status-bar] show failed for state={state}: {e}"),
    }
    hud_manager::configure_window(win);
    let _ = hud_manager::show_panel(app);
    if resync {
        diag::breadcrumb(format!("status_bar:present:{state}:resync"));
        emit_status_bar_resync(app, state);
    }
    diag::breadcrumb(format!("status_bar:present:{state}:end"));
}

#[cfg(target_os = "macos")]
pub(crate) fn schedule_present_status_bar_macos(
    app: &tauri::AppHandle,
    win: &tauri::WebviewWindow,
    state: &str,
    resync: bool,
) {
    let app_for_main = app.clone();
    let app_in_closure = app_for_main.clone();
    let win = win.clone();
    let state = state.to_string();
    diag::breadcrumb(format!("status_bar:schedule_present:{state}"));
    if let Err(e) = run_on_main_guarded(&app_for_main, "status_bar.present", move || {
        present_status_bar_macos_on_main(&app_in_closure, &win, &state, resync);
    }) {
        tracing::warn!("[status-bar] schedule present on main thread failed: {e}");
    }
}

fn present_status_bar_native(
    app: &tauri::AppHandle,
    reason: &str,
    resync: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return hud_manager::MacHudManager::new(app).present(reason, resync);
    }

    #[cfg(not(target_os = "macos"))]
    {
        if app.get_webview_window("status-bar").is_none() {
            // The status-bar window is pre-created at startup. If it is somehow missing,
            // do NOT create it here on Windows: this fn is reachable from the
            // present_status_bar / set_status_bar_persistent IPC commands, and creating
            // a WebView2 window from inside an IPC handler deadlocks Windows IPC
            // (wry #583 — the same class that wedged End-meeting). macOS's NSPanel path
            // is safe.
            tracing::warn!(
                "[status-bar] window missing — not recreating from an IPC path on Windows"
            );
            return Err("status-bar window not available".to_string());
        }
        let win = app
            .get_webview_window("status-bar")
            .ok_or_else(|| "status-bar window not found".to_string())?;
        tracing::debug!("[status-bar] native present reason={reason} resync={resync}");

        // Position the HUD before showing — the macOS path repositions via the
        // panel presenter, but the native path must do it explicitly or the
        // window appears at its stale/default origin.
        reposition_status_bar(app, &win);
        win.set_always_on_top(true)
            .map_err(|e| format!("set_always_on_top failed: {e}"))?;
        let _ = win.set_visible_on_all_workspaces(true); // no-op on Windows; best-effort
        win.show()
            .map_err(|e| format!("show status bar failed: {e}"))?;
        if resync {
            emit_status_bar_resync(app, reason);
        }

        Ok(())
    }
}

// ── Keystroke reconstruction (edit detection for AX-blind apps) ──────────────
//
// The existing CGEventTap in the hotkey crate is extended to also capture
// kCGEventKeyDown events into a rolling buffer.  watch_for_edit notes
// Instant::now() before watching, then replays all buffered keystrokes
// timestamped AFTER that instant against the known pasted text.
//
// This is the same technique Wispr Flow uses — no second CGEventTap needed.

/// Apply buffered keystrokes to reconstruct the final text in an AX-blind app.
///
/// `initial` is the text we pasted.  Events are filtered to those that arrived
/// after `since`.  Returns `None` only if reconstruction is truly unreliable
/// Show a macOS native notification.
///
/// Two-path notifier.
///
/// The macOS notification banner shows the icon of the *process that posted
/// it*, which is why we have two paths:
///
/// 1. **Production (.app bundle):** use `tauri-plugin-notification`. The
///    plugin posts via the app's own bundle, so the banner shows the AirNote
///    icon (icon.icns from tauri.conf.json's `bundle.icon`). This is the
///    only path that gets the brand on the banner.
///
/// 2. **Dev (raw debug binary):** the plugin silently no-ops because
///    `mac-notification-sys` requires a registered bundle identifier.
///    Fall back to `osascript`, which always shows a banner but uses the
///    Script Editor icon (we can't override it via AppleScript). At least
///    the user sees *that* something happened in dev.
///
/// We detect "running inside a .app" by inspecting the executable path
/// (`*.app/Contents/MacOS/*`). Cheap, no syscalls, no environment sniffing.
#[cfg(target_os = "macos")]
fn notify_macos(app: &tauri::AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;

    if is_bundled_app() {
        match app.notification().builder().title(title).body(body).show() {
            Ok(_) => {
                tracing::info!("[notify] plugin sent (AirNote icon): {title}");
                return;
            }
            Err(e) => {
                tracing::warn!("[notify] plugin failed: {e} — falling back to osascript");
            }
        }
    }
    osa_fallback(title, body);
}

#[cfg(target_os = "macos")]
fn is_bundled_app() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(|s| s.to_string()))
        .map(|s| s.contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn osa_fallback(title: &str, body: &str) {
    use std::process::{Command, Stdio};
    // AppleScript string literals: backslash-escape `\` and `"`.
    let title_esc = title.replace('\\', "\\\\").replace('"', "\\\"");
    let body_esc = body.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(r#"display notification "{body_esc}" with title "{title_esc}""#);
    match Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(_) => tracing::info!("[notify] osa sent (no icon): {title}"),
        Err(e) => tracing::warn!("[notify] osascript spawn failed: {e}"),
    }
}

// Windows / Linux: post via the cross-platform notification plugin. On Windows
// this shows an Action Center toast — it requires the app to be installed (the
// NSIS installer registers the Start-Menu shortcut + AppUserModelID), which is
// why nothing appears for an unpackaged/dev run but works once installed.
#[cfg(not(target_os = "macos"))]
fn notify_macos(app: &tauri::AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    match app.notification().builder().title(title).body(body).show() {
        Ok(_) => tracing::info!("[notify] plugin sent: {title}"),
        Err(e) => tracing::warn!("[notify] plugin failed: {e}"),
    }
}

/// Translate a raw pipeline error string into one short human sentence.
///
/// Keep raw diagnostic strings out of the UI. This maps common local recording
/// and polish failures to short, calm user-facing messages.
fn emit_voice_error_quiet(app: &tauri::AppHandle, raw: &str) {
    tracing::error!("[pipeline] error: {raw}");
    let human = humanize_error(raw);
    let _ = app.emit(
        "voice-error",
        serde_json::json!({
            "message": human,
            "raw_error": raw,
            "auto_hide_ms": 4000,
        }),
    );
}

fn humanize_error(raw: &str) -> String {
    let raw = strip_voice_error_already_emitted(raw);
    let lower = raw.to_lowercase();

    if lower.contains("empty transcript") || lower.contains("nothing spoken") {
        return "Couldn't hear anything".into();
    }
    if lower.contains("recording interrupted") {
        return "Recording interrupted".into();
    }
    if lower.contains("local speech model is required") || lower.contains("speech model") {
        return "Download the local speech model first".into();
    }
    if lower.contains("local speech") || lower.contains("transcript") {
        return "Speech recognition failed — try again".into();
    }
    if lower.contains("openai")
        && (lower.contains("not connected") || lower.contains("401") || lower.contains("403"))
    {
        return "OpenAI not connected".into();
    }
    if lower.contains("groq") && (lower.contains("401") || lower.contains("403")) {
        return "Groq key invalid".into();
    }
    if lower.contains("groq") && lower.contains("not set") {
        return "Groq key missing".into();
    }
    if lower.contains("gemini") && (lower.contains("401") || lower.contains("403")) {
        return "Gemini key invalid".into();
    }
    if lower.contains("together") && (lower.contains("rate limit") || lower.contains("429")) {
        return "Together AI rate limit hit".into();
    }
    if lower.contains("rate") || lower.contains("429") {
        return "Rate limited — wait a moment".into();
    }
    if lower.contains("preferences not found") {
        return "Loading settings…".into();
    }
    if lower.contains("missing_api_keys") || lower.contains("api keys required") {
        return "API keys needed".into();
    }
    if lower.contains("timeout") || lower.contains("timed out") {
        return "Connection timed out".into();
    }
    if lower.contains("dns") || lower.contains("unreachable") || lower.contains("failed to connect")
    {
        return "Network unreachable".into();
    }
    if lower.contains("backend") && (lower.contains("not started") || lower.contains("lock")) {
        return "Backend starting…".into();
    }

    let short: String = raw.chars().take(30).collect();
    format!("Failed: {short}")
}

fn mark_voice_error_already_emitted(raw: &str) -> String {
    format!("{VOICE_ERROR_ALREADY_EMITTED_PREFIX}{raw}")
}

fn is_voice_error_already_emitted(raw: &str) -> bool {
    raw.starts_with(VOICE_ERROR_ALREADY_EMITTED_PREFIX)
}

fn strip_voice_error_already_emitted(raw: &str) -> &str {
    raw.strip_prefix(VOICE_ERROR_ALREADY_EMITTED_PREFIX)
        .unwrap_or(raw)
}

// ── Managed state ─────────────────────────────────────────────────────────────

/// Holds the local recording state machine.
pub(crate) struct SharedApp(Arc<Mutex<DesktopApp>>);

/// Holds the backend endpoint (url + secret). None until daemon starts.
struct BackendState(Arc<Mutex<Option<BackendEndpoint>>>);

/// Owns the BackendHandle (and its Child) for the lifetime of the app.
/// When Tauri drops managed state on exit, Drop fires → SIGTERM → SIGKILL.
struct BackendHandleState(Mutex<Option<backend::BackendHandle>>);

fn backend_health_ok(client: &reqwest::blocking::Client, ep: &BackendEndpoint) -> bool {
    client
        .get(format!("{}/v1/health/ping", ep.url))
        .timeout(Duration::from_secs(2))
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

fn backend_watchdog_app_is_idle(app: &tauri::AppHandle) -> bool {
    app.try_state::<SharedApp>()
        .map(|shared| {
            shared
                .0
                .lock()
                .map(|d| d.state == desktop::AppState::Idle)
                .unwrap_or_else(|p| p.into_inner().state == desktop::AppState::Idle)
        })
        .unwrap_or(true)
}

fn install_backend_handle(
    app: &tauri::AppHandle,
    handle: backend::BackendHandle,
) -> BackendEndpoint {
    let ep = handle.endpoint();
    if let Some(pid) = handle.pid() {
        backend_guard::write_pid_file(pid);
    }
    match app.state::<BackendState>().0.lock() {
        Ok(mut slot) => *slot = Some(ep.clone()),
        Err(p) => *p.into_inner() = Some(ep.clone()),
    }
    match app.state::<BackendHandleState>().0.lock() {
        Ok(mut slot) => *slot = Some(handle),
        Err(p) => *p.into_inner() = Some(handle),
    }
    ep
}

fn start_backend_watchdog(app: tauri::AppHandle, using_external_backend: bool) {
    if using_external_backend {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("backend-watchdog".to_string())
        .spawn(move || {
            let client = reqwest::blocking::Client::new();
            let mut failed_checks = 0u32;
            loop {
                std::thread::sleep(Duration::from_secs(10));

                if !backend_watchdog_app_is_idle(&app) {
                    failed_checks = 0;
                    continue;
                }

                let current = app
                    .state::<BackendState>()
                    .0
                    .lock()
                    .map(|slot| slot.clone())
                    .unwrap_or_else(|p| p.into_inner().clone());
                if current
                    .as_ref()
                    .is_some_and(|ep| backend_health_ok(&client, ep))
                {
                    failed_checks = 0;
                    continue;
                }

                failed_checks += 1;
                if failed_checks < 3 {
                    continue;
                }

                tracing::error!(
                    "[backend-watchdog] backend unreachable for {failed_checks} checks — respawning"
                );
                diag::breadcrumb("backend_watchdog:respawn");
                said_core::reporter::report_event(
                    "backend.unreachable",
                    said_core::reporter::Severity::Error,
                    serde_json::json!({ "failed_checks": failed_checks }),
                );

                match backend::spawn() {
                    Ok(handle) => {
                        let ep_notifications = handle.endpoint();
                        let ep = install_backend_handle(&app, handle);
                        failed_checks = 0;
                        tracing::warn!("[backend-watchdog] backend recovered at {}", ep.url);
                        said_core::reporter::report_event(
                            "backend.recovered",
                            said_core::reporter::Severity::Warning,
                            serde_json::json!({ "url": ep.url }),
                        );
                        let _ = app.emit("backend-recovered", serde_json::json!({ "url": ep.url }));
                        start_runtime_notification_listener(app.clone(), ep_notifications);
                    }
                    Err(e) => {
                        tracing::error!("[backend-watchdog] respawn failed: {e}");
                        said_core::reporter::report_event(
                            "backend.respawn_failed",
                            said_core::reporter::Severity::Error,
                            serde_json::json!({ "error": e.to_string() }),
                        );
                    }
                }
            }
        });
}

#[derive(Clone)]
struct EditWatcherControl {
    generation: u64,
    cancel: CancellationToken,
    finalize: CancellationToken,
}

fn new_edit_watcher_control(generation: u64) -> EditWatcherControl {
    EditWatcherControl {
        generation,
        cancel: CancellationToken::new(),
        finalize: CancellationToken::new(),
    }
}

fn edit_watch_crossed_app_boundary(initial_pid: Option<i32>, now_pid: Option<i32>) -> bool {
    matches!((initial_pid, now_pid), (Some(initial), Some(now)) if initial != now)
}

fn edit_watcher_generation_is_current(
    current: Option<&EditWatcherControl>,
    task_generation: u64,
) -> bool {
    current.is_some_and(|control| control.generation == task_generation)
}

/// Owns the currently running post-paste edit watcher. A new recording asks the
/// previous watcher to finalize; unrelated replacement actions may hard-cancel.
struct EditWatcherState(Mutex<Option<EditWatcherControl>>);
static EDIT_WATCHER_GENERATION: AtomicU64 = AtomicU64::new(0);

/// The frontmost app PID when recording started.
///
/// We lock this before showing/updating our own UI, then the post-paste edit
/// watcher reads that app directly instead of chasing whatever system focus is
/// later. This matches OpenWhispr's target-PID monitoring model.
struct EditTargetState(Mutex<Option<i32>>);

/// Screen context captured at recording start — the text already in the
/// focused field.  Sent to the backend so the LLM can use surrounding text
/// as a hint for smarter STT corrections (e.g. if the field already mentions
/// "MACOBS", the LLM knows "main corps" is likely "MACOBS").
struct ScreenContextState(Mutex<Option<String>>);

async fn stt_pre_transcript(
    wav: &[u8],
    language: &str,
) -> Result<dictation_stt::PreTranscript, String> {
    let started = std::time::Instant::now();
    let transcript = dictation_stt::transcribe_wav_bytes(wav, language).await?;
    tracing::info!(
        "[finish] speech transcript ready in {}ms provider={} chars={} words={}",
        started.elapsed().as_millis(),
        dictation_stt::provider_name(),
        transcript.transcript.len(),
        transcript.transcript.split_whitespace().count()
    );
    Ok(transcript)
}

async fn finish_live_nemotron_transcript(
    mut live_run: LiveNemotronRun,
) -> Result<dictation_stt::PreTranscript, String> {
    let started = std::time::Instant::now();
    if let Some(drain_done_rx) = live_run.drain_done_rx.take() {
        // The drain is the single producer of live PCM commands. Wait for it
        // before committing so the final release cannot overtake its last
        // recorder chunk through a separate mpsc sender.
        tokio::task::spawn_blocking(move || drain_done_rx.recv_timeout(Duration::from_secs(2)))
            .await
            .map_err(|error| format!("live audio drain task failed: {error}"))?
            .map_err(|_| "Live audio capture did not finish cleanly. Try again.".to_string())?;
    }
    live_run
        .controller
        .commit()
        .await
        .map_err(|error| error.to_string())?;
    let transcript = tokio::time::timeout(Duration::from_secs(75), live_run.final_rx)
        .await
        .map_err(|_| {
            "The speech service didn't respond within 75s — check your connection and try again."
                .to_string()
        })?
        .map_err(|_| "Live speech transcription stopped unexpectedly. Try again.".to_string())??;
    tracing::info!(
        "[finish] live speech transcript finalized in {}ms provider={} chars={} words={}",
        started.elapsed().as_millis(),
        dictation_stt::provider_name(),
        transcript.transcript.len(),
        transcript.transcript.split_whitespace().count()
    );
    Ok(transcript)
}

/// Stores the most-recently polished text. Populated after every voice/text polish;
/// cleared after it's pasted via the global paste-latest hotkey or command.
struct LatestResult(std::sync::Arc<Mutex<Option<String>>>);

fn paste_latest_hotkey_label() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Ctrl+Cmd+V"
    }
    #[cfg(target_os = "windows")]
    {
        "Ctrl+Alt+V"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Paste latest"
    }
}

#[derive(Clone, Debug)]
enum LastRepairStage {
    None,
    FastRepair,
}

#[derive(Clone, Debug)]
struct LastVoiceAction {
    recording_id: String,
    audio_id: Option<String>,
    raw_transcript: String,
    enriched_transcript: Option<String>,
    polished: String,
    output_language: String,
    target_app: Option<String>,
    completed_at_ms: i64,
    last_repair_stage: LastRepairStage,
    last_repair_at_ms: Option<i64>,
}

#[derive(Clone, Debug)]
struct LastFailedVoiceAction {
    audio_id: String,
    raw_error: String,
    error_code: Option<String>,
    target_app: Option<String>,
    client_run_id: Option<String>,
    message_polish_mode: bool,
    failed_at_ms: i64,
}

#[derive(Clone, Debug)]
struct LastTextTransformAction {
    source_text: String,
    polished: String,
    tone: String,
    completed_at_ms: i64,
}

#[derive(Clone, Debug)]
enum LastAction {
    Voice(LastVoiceAction),
    TextTransform(LastTextTransformAction),
}

struct LastActionState(Mutex<Option<LastAction>>);
struct LastFailedVoiceActionState(Mutex<Option<LastFailedVoiceAction>>);

struct PerformanceState(Mutex<sysinfo::System>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordingRoute {
    Normal,
    Meeting,
    /// Ctrl hold-to-talk: transcribe + polish, then send to Divo instead of pasting.
    Divo,
    /// Developer Problem Command: transcribe, resolve local project context, solve, final-paste only.
    Problem,
}

struct RecordingRouteState(Mutex<Option<RecordingRoute>>);

#[derive(Clone)]
struct PendingProblemCommand {
    transcript: String,
    screen_context: Option<String>,
    client_run_id: Option<String>,
    edit_target_pid: Option<i32>,
    candidates: Vec<developer_context::DeveloperMatchedProject>,
    created_at_ms: i64,
}

struct PendingProblemState(Mutex<Option<PendingProblemCommand>>);

struct LongDictationState {
    locked: Arc<AtomicBool>,
    pending_lock: Arc<AtomicBool>,
    stop_consumed: Arc<AtomicBool>,
    finishing: Arc<AtomicBool>,
}

impl LongDictationState {
    fn new() -> Self {
        Self {
            locked: Arc::new(AtomicBool::new(false)),
            pending_lock: Arc::new(AtomicBool::new(false)),
            stop_consumed: Arc::new(AtomicBool::new(false)),
            finishing: Arc::new(AtomicBool::new(false)),
        }
    }

    fn reset(&self) -> bool {
        let was_locked = self.locked.swap(false, Ordering::SeqCst);
        let had_pending_lock = self.pending_lock.swap(false, Ordering::SeqCst);
        let had_consumed_stop = self.stop_consumed.swap(false, Ordering::SeqCst);
        let was_finishing = self.finishing.swap(false, Ordering::SeqCst);
        was_locked || had_pending_lock || had_consumed_stop || was_finishing
    }

    fn clear_recording_lock(&self) -> bool {
        let was_locked = self.locked.swap(false, Ordering::SeqCst);
        let had_pending_lock = self.pending_lock.swap(false, Ordering::SeqCst);
        was_locked || had_pending_lock
    }

    fn clear_finishing(&self) -> bool {
        let had_consumed_stop = self.stop_consumed.swap(false, Ordering::SeqCst);
        let was_finishing = self.finishing.swap(false, Ordering::SeqCst);
        had_consumed_stop || was_finishing
    }
}

#[derive(Clone, Serialize)]
struct MeetingSttStatus {
    active: bool,
    muted: bool,
    capture_running: bool,
    speaker_reference_available: bool,
    echo_gate_active: bool,
    local_speech_active: bool,
    last_gate_reason: String,
}

/// Meeting-mode flags. Active means the live meeting view owns the recorder;
/// muted prevents pause-detection restarts while the user is holding Fn.
struct MeetingModeState {
    active: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
    echo_gate: Arc<echo_gate::EchoGateShared>,
}

impl MeetingModeState {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
            echo_gate: echo_gate::EchoGateShared::new(),
        }
    }

    fn enter(&self) -> bool {
        self.muted.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        !self.active.swap(true, Ordering::SeqCst)
    }

    fn exit(&self) -> bool {
        self.muted.store(false, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.echo_gate.stop_reference();
        self.active.swap(false, Ordering::SeqCst)
    }

    fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    fn is_muted(&self) -> bool {
        self.muted.load(Ordering::SeqCst)
    }

    fn capture_enabled(&self) -> bool {
        self.is_active() && !self.is_muted()
    }

    fn set_muted(&self, muted: bool) {
        if self.muted.swap(muted, Ordering::SeqCst) != muted {
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
        if muted {
            self.echo_gate.stop_reference();
        }
    }

    fn status(&self) -> MeetingSttStatus {
        let echo_status = self.echo_gate.status();
        MeetingSttStatus {
            active: self.is_active(),
            muted: self.is_muted(),
            capture_running: self.capture_enabled(),
            speaker_reference_available: echo_status.speaker_reference_available,
            echo_gate_active: echo_status.echo_gate_active,
            local_speech_active: echo_status.local_speech_active,
            last_gate_reason: echo_status.last_gate_reason,
        }
    }

    fn ensure_echo_reference(&self) -> Result<(), String> {
        if self.echo_gate.is_filter_available() {
            return Ok(());
        }
        self.echo_gate.start_reference()
    }
}

fn emit_meeting_stt_status(app: &tauri::AppHandle) {
    if let Some(meeting) = app.try_state::<MeetingModeState>() {
        let _ = app.emit("meeting-stt-state", meeting.status());
    }
}

fn restore_speaker_suppression(app: &tauri::AppHandle, reason: &str) {
    #[cfg(target_os = "macos")]
    if let Some(guard) = app.try_state::<speaker_suppression::SpeakerSuppressionGuard>() {
        guard.restore(reason);
    }
}

/// Hot-path cache: local speech language setting.
///
/// Populated once when the backend becomes ready; refreshed in the background
/// whenever preferences change. The recording finish task reads from this cache
/// so local transcription does not wait on a preferences request.
struct HotPathCache(Arc<tokio::sync::RwLock<HotPathCacheInner>>);

#[derive(Default, Clone)]
struct HotPathCacheInner {
    /// User's speech language setting (e.g. "hi", "multi", "auto").
    language: String,
}

/// Generation counter for status-bar idle hide timers.
/// Each idle sync increments this; a timer whose generation no longer matches is silently dropped.
pub(crate) struct StatusBarHideGen(Arc<AtomicU64>);

/// True while the status bar is showing a user-action-required notification.
/// Idle app-state syncs must not hide the native window until the frontend
/// explicitly clears this hold.
struct StatusBarPersistentHold(AtomicBool);

/// True while ⇧⌘/ placement mode is active (drag to reposition HUD).
struct StatusBarPlacementActive(AtomicBool);

/// Whether the status bar should currently accept mouse clicks. Single source of
/// truth so every native show/tune re-applies the real state instead of hardcoding
/// click-through: a `present` while the HUD is already interactive (e.g.
/// divo_streaming → divo_ready) would otherwise silently re-disable clicks, since
/// the frontend only re-asserts interactivity when the flag actually changes.
struct StatusBarInteractive(AtomicBool);

/// Active dictation session id + generation for stale-result guards and logging.
struct RecordingSessionState {
    generation: AtomicU64,
    active_id: Mutex<Option<String>>,
    processing_id: Mutex<Option<String>>,
}

/// One active Together Nemotron session, captured at recording start. It is
/// separate from `RecordingSessionState` because its command channel must stay
/// available to the recorder drain while the final transcript receiver moves to
/// the release pipeline.
struct LiveNemotronRun {
    generation: u64,
    controller: asr_cloud::LiveTranscriptionController,
    drain_done_rx: Option<std::sync::mpsc::Receiver<()>>,
    final_rx: tokio::sync::oneshot::Receiver<Result<dictation_stt::PreTranscript, String>>,
}

struct LiveNemotronState(Mutex<Option<LiveNemotronRun>>);

impl RecordingSessionState {
    fn begin(&self) -> (u64, String) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let id = uuid::Uuid::new_v4().to_string();
        if let Ok(mut guard) = self.active_id.lock() {
            *guard = Some(id.clone());
        }
        if let Ok(mut guard) = self.processing_id.lock() {
            *guard = None;
        }
        tracing::info!("[record] session begin id={id} generation={generation}");
        (generation, id)
    }

    fn end(&self) -> Option<(u64, String)> {
        let generation = self.generation.load(Ordering::SeqCst);
        let id = self.active_id.lock().ok()?.take()?;
        if let Ok(mut guard) = self.processing_id.lock() {
            *guard = Some(id.clone());
        }
        tracing::info!("[record] session end id={id} generation={generation}");
        Some((generation, id))
    }

    fn current(&self) -> Option<(u64, String)> {
        let generation = self.generation.load(Ordering::SeqCst);
        let id = self.active_id.lock().ok()?.clone()?;
        Some((generation, id))
    }

    fn id_for_snapshot_state(&self, state: &str) -> Option<String> {
        match state {
            "recording" => self.active_id.lock().ok()?.clone(),
            "processing" => self
                .processing_id
                .lock()
                .ok()
                .and_then(|id| id.clone())
                .or_else(|| self.active_id.lock().ok()?.clone()),
            _ => None,
        }
    }
}

impl Default for RecordingSessionState {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            active_id: Mutex::new(None),
            processing_id: Mutex::new(None),
        }
    }
}

fn start_live_nemotron_session(app: &tauri::AppHandle, generation: u64, run_id: String) {
    if !dictation_stt::uses_live_nemotron() {
        return;
    }

    let (controller, input) = asr_cloud::live_transcription_input();
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (final_tx, final_rx) = tokio::sync::oneshot::channel();
    let app_for_events = app.clone();
    let run_for_events = run_id.clone();

    // Together's deltas replace the prior hypothesis. They are strictly HUD
    // preview; the focused app still receives only server-polished output.
    tauri::async_runtime::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let still_recording = app_for_events
                .try_state::<SharedApp>()
                .and_then(|shared| {
                    shared
                        .0
                        .lock()
                        .ok()
                        .map(|app_state| app_state.state == desktop::AppState::Recording)
                })
                .unwrap_or(false);
            let generation_is_current = app_for_events
                .try_state::<RecordingSessionState>()
                .map(|session| session.generation.load(Ordering::SeqCst) == generation)
                .unwrap_or(false);
            if !still_recording || !generation_is_current {
                continue;
            }
            match event {
                asr_cloud::LiveTranscriptEvent::Ready => {
                    tracing::info!(
                        run_id = %run_for_events,
                        generation,
                        "[nemotron_live] websocket session ready while recording"
                    );
                    emit_voice_status(
                        &app_for_events,
                        "live_stt",
                        Some("Listening…"),
                        Some(&run_for_events),
                    );
                }
                asr_cloud::LiveTranscriptEvent::Delta { transcript } => {
                    emit_voice_status(
                        &app_for_events,
                        "live_stt",
                        Some(&transcript),
                        Some(&run_for_events),
                    );
                }
            }
        }
    });

    tauri::async_runtime::spawn(async move {
        let result = dictation_stt::transcribe_live_nemotron(input, event_tx).await;
        let _ = final_tx.send(result);
    });

    let live_state = app.state::<LiveNemotronState>();
    let mut guard = live_state
        .0
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    // A new recording is allowed to supersede an abandoned old one. Dropping
    // its controller closes the command channel once its recorder drain ends.
    *guard = Some(LiveNemotronRun {
        generation,
        controller,
        drain_done_rx: None,
        final_rx,
    });
    tracing::info!(
        run_id = %run_id,
        generation,
        "[nemotron_live] session opening at recording start"
    );
}

fn live_nemotron_controller(
    app: &tauri::AppHandle,
    generation: u64,
) -> Option<asr_cloud::LiveTranscriptionController> {
    let state = app.try_state::<LiveNemotronState>()?;
    let guard = state.0.lock().ok()?;
    guard
        .as_ref()
        .filter(|run| run.generation == generation)
        .map(|run| run.controller.clone())
}

fn attach_live_nemotron_drain(
    app: &tauri::AppHandle,
    generation: u64,
    drain_done_rx: std::sync::mpsc::Receiver<()>,
) {
    let Some(state) = app.try_state::<LiveNemotronState>() else {
        return;
    };
    let Ok(mut guard) = state.0.lock() else {
        return;
    };
    if let Some(run) = guard.as_mut().filter(|run| run.generation == generation) {
        run.drain_done_rx = Some(drain_done_rx);
    }
}

fn take_live_nemotron_run(
    app: &tauri::AppHandle,
    generation: Option<u64>,
) -> Option<LiveNemotronRun> {
    let generation = generation?;
    let state = app.try_state::<LiveNemotronState>()?;
    let mut guard = state.0.lock().ok()?;
    if guard
        .as_ref()
        .is_some_and(|run| run.generation == generation)
    {
        guard.take()
    } else {
        None
    }
}

fn discard_live_nemotron_run(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<LiveNemotronState>() else {
        return;
    };
    if let Ok(mut guard) = state.0.lock() {
        if guard.take().is_some() {
            tracing::debug!("[nemotron_live] session discarded before commit");
        }
    }
}

fn snapshot_for_ui(app: &tauri::AppHandle, snap: &AppSnapshot) -> AppSnapshot {
    let mut snap = snap.clone();
    snap.recording_id = app
        .try_state::<RecordingSessionState>()
        .and_then(|session| session.id_for_snapshot_state(&snap.state));
    snap
}

fn emit_app_state(app: &tauri::AppHandle, snap: &AppSnapshot) {
    let ui_snap = snapshot_for_ui(app, snap);
    let _ = app.emit("app-state", &ui_snap);
}

fn emit_voice_status(
    app: &tauri::AppHandle,
    phase: &str,
    transcript: Option<&str>,
    run_id: Option<&str>,
) {
    let _ = app.emit(
        "voice-status",
        serde_json::json!({
            "phase": phase,
            "transcript": transcript,
            "run_id": run_id,
            "recording_id": run_id,
        }),
    );
}

fn emit_voice_done(app: &tauri::AppHandle, done: &api::PolishDone, run_id: Option<&str>) {
    let mut payload = serde_json::to_value(done).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("run_id".to_string(), serde_json::json!(run_id));
    }
    let _ = app.emit("voice-done", payload);
}

pub(crate) fn status_bar_persistent_hold(app: &tauri::AppHandle) -> bool {
    app.try_state::<StatusBarPersistentHold>()
        .map(|s| s.0.load(Ordering::SeqCst))
        .unwrap_or(false)
}

/// Whether the status bar currently wants to accept clicks. Defaults to false
/// (click-through) when the state hasn't been registered yet.
#[cfg(not(target_os = "macos"))]
fn apply_status_bar_interactive_state(win: &tauri::WebviewWindow, interactive: bool) {
    let _ = win.set_ignore_cursor_events(!interactive);
    tracing::debug!("[status-bar] interactive={interactive}");
}

fn set_status_bar_interactive_state(app: &tauri::AppHandle, interactive: bool) {
    #[cfg(target_os = "macos")]
    {
        hud_manager::MacHudManager::new(app).set_interactive(interactive);
        return;
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(state) = app.try_state::<StatusBarInteractive>() {
            state.0.store(interactive, Ordering::SeqCst);
        }
        if let Some(win) = app.get_webview_window("status-bar") {
            apply_status_bar_interactive_state(&win, interactive);
        }
    }
}

fn placement_mode_active(app: &tauri::AppHandle) -> bool {
    app.try_state::<StatusBarPlacementActive>()
        .map(|s| s.0.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn set_placement_mode_active(app: &tauri::AppHandle, active: bool) {
    if let Some(s) = app.try_state::<StatusBarPlacementActive>() {
        s.0.store(active, Ordering::Relaxed);
    }
}

#[derive(serde::Serialize)]
struct DebugLogs {
    desktop_path: String,
    backend_path: String,
    breadcrumb_path: String,
    desktop: String,
    backend: String,
    breadcrumbs: String,
    combined: String,
    truncated: bool,
}

#[derive(serde::Serialize)]
struct ProcessPerf {
    pid: u32,
    name: String,
    cpu_percent: f32,
    memory_bytes: u64,
    virtual_memory_bytes: u64,
}

#[derive(serde::Serialize)]
struct GpuPerf {
    available: bool,
    label: String,
    utilization_percent: Option<f32>,
    memory_bytes: Option<u64>,
}

#[derive(serde::Serialize)]
struct PerformanceSnapshot {
    timestamp_ms: i64,
    cpu_percent: f32,
    physical_core_count: Option<usize>,
    total_memory_bytes: u64,
    used_memory_bytes: u64,
    available_memory_bytes: u64,
    total_swap_bytes: u64,
    used_swap_bytes: u64,
    desktop: Option<ProcessPerf>,
    backend: Option<ProcessPerf>,
    gpu: GpuPerf,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn voice_audio_dir() -> std::path::PathBuf {
    let base = dirs::data_local_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
    base.join("VoicePolish").join("audio")
}

fn saved_voice_audio_path(audio_id: &str) -> Result<std::path::PathBuf, String> {
    uuid::Uuid::parse_str(audio_id).map_err(|_| "invalid saved audio id".to_string())?;
    Ok(voice_audio_dir().join(format!("{audio_id}.wav")))
}

fn saved_voice_audio_exists(audio_id: &str) -> bool {
    saved_voice_audio_path(audio_id)
        .map(|path| path.is_file())
        .unwrap_or(false)
}

fn latest_saved_voice_audio_id() -> Option<String> {
    let entries = std::fs::read_dir(voice_audio_dir()).ok()?;
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wav") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            if uuid::Uuid::parse_str(&stem).is_err() {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, stem))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, audio_id)| audio_id)
}

fn parse_record_hotkey(raw: &str) -> hotkey::RecordHotkey {
    // Fn/Globe is macOS-only; on Windows the win_hotkey backend has no VK for it
    // (target_vk(Function)=None) so hold-to-record would be silently dead. A "fn"
    // pref can still arrive via account sync from a Mac profile — degrade to Caps
    // Lock on non-macOS instead of leaving recording broken.
    #[cfg(not(target_os = "macos"))]
    if raw == "fn" {
        return hotkey::RecordHotkey::CapsLock;
    }
    // Preset keys + any sided modifier are resolved from their stable string id in
    // the hotkey crate (which owns the platform keycodes/masks/VKs). Unknown ids
    // fall back to Caps Lock.
    hotkey::RecordHotkey::from_id(raw).unwrap_or(hotkey::RecordHotkey::CapsLock)
}

fn cache_last_voice_action(
    app: &tauri::AppHandle,
    done: &api::PolishDone,
    repair_stage: LastRepairStage,
) {
    let output_language = done
        .output_language
        .clone()
        .unwrap_or_else(|| "hinglish".into());
    let action = LastAction::Voice(LastVoiceAction {
        recording_id: done.recording_id.clone(),
        audio_id: done.audio_id.clone(),
        raw_transcript: done.transcript.clone(),
        enriched_transcript: done.enriched_transcript.clone(),
        polished: done.polished.clone(),
        output_language,
        target_app: done.target_app.clone(),
        completed_at_ms: now_ms(),
        last_repair_stage: repair_stage.clone(),
        last_repair_at_ms: match repair_stage {
            LastRepairStage::FastRepair => Some(now_ms()),
            LastRepairStage::None => None,
        },
    });
    if let Ok(mut guard) = app.state::<LastActionState>().0.lock() {
        *guard = Some(action);
    }
    if let Some(state) = app.try_state::<LastFailedVoiceActionState>() {
        if let Ok(mut guard) = state.0.lock() {
            *guard = None;
        }
    }
}

fn cache_last_failed_voice_action(
    app: &tauri::AppHandle,
    audio_id: Option<String>,
    raw_error: &str,
    error_code: Option<String>,
    target_app: Option<String>,
    client_run_id: Option<String>,
    message_polish_mode: bool,
) {
    let Some(audio_id) = audio_id.filter(|id| saved_voice_audio_exists(id)) else {
        tracing::warn!(
            "[retry] failed voice action not cached — no saved audio for error_code={}",
            error_code.as_deref().unwrap_or("none")
        );
        return;
    };
    let action = LastFailedVoiceAction {
        audio_id,
        raw_error: raw_error.to_string(),
        error_code,
        target_app,
        client_run_id,
        message_polish_mode,
        failed_at_ms: now_ms(),
    };
    tracing::info!(
        "[retry] cached failed voice action audio_id={} message_polish={} client_run_id={} error_code={} target_app={}",
        action.audio_id,
        action.message_polish_mode,
        action.client_run_id.as_deref().unwrap_or("none"),
        action.error_code.as_deref().unwrap_or("none"),
        action.target_app.as_deref().unwrap_or("none"),
    );
    if let Ok(mut guard) = app.state::<LastFailedVoiceActionState>().0.lock() {
        *guard = Some(action);
    }
}

fn failed_voice_mode_for_audio(app: &tauri::AppHandle, audio_id: &str) -> Option<bool> {
    app.try_state::<LastFailedVoiceActionState>()
        .and_then(|state| state.0.lock().ok()?.clone())
        .filter(|action| action.audio_id == audio_id)
        .map(|action| action.message_polish_mode)
}

fn cache_last_text_transform(
    app: &tauri::AppHandle,
    source_text: String,
    polished: String,
    tone: String,
) {
    let action = LastAction::TextTransform(LastTextTransformAction {
        source_text,
        polished,
        tone,
        completed_at_ms: now_ms(),
    });
    if let Ok(mut guard) = app.state::<LastActionState>().0.lock() {
        *guard = Some(action);
    }
}

/// Lightweight cache of tray-relevant prefs so `sync_tray` never needs async.
struct TrayCache(Mutex<TrayCacheInner>);
struct TrayCacheInner {
    custom_prompt: Option<String>,
    output_language: String, // "hinglish" | "english" | "hindi"
    record_hotkey: String,   // "caps_lock" | "right_option" | "fn"
}
impl Default for TrayCacheInner {
    fn default() -> Self {
        Self {
            custom_prompt: None,
            output_language: "hinglish".into(),
            record_hotkey: "caps_lock".into(),
        }
    }
}

// ── Tray helpers ──────────────────────────────────────────────────────────────

/// Short status text that appears next to the brand icon in the menu bar.
/// Empty when idle (icon alone).
fn tray_title(state: &str) -> &'static str {
    match state {
        "recording" => " ● REC",
        _ => "",
    }
}

/// Build the dynamic tray menu.
/// Re-run on every state change so recording label and language checkmarks stay in sync.
fn build_tray_menu(
    app: &tauri::AppHandle,
    snap: &AppSnapshot,
    _custom_prompt: Option<&str>,
    _output_language: &str,
) -> Result<Menu<tauri::Wry>, tauri::Error> {
    build_tray_menu_for_state(app, &snap.state)
}

fn build_tray_menu_for_state(
    app: &tauri::AppHandle,
    state: &str,
) -> Result<Menu<tauri::Wry>, tauri::Error> {
    // ── 1. Toggle recording (state-aware label + enabled) ──────────────
    let toggle_label = match state {
        "recording" => "Stop recording",
        _ => "Start recording",
    };
    let toggle_enabled = state == "idle";
    let toggle = MenuItem::with_id(
        app,
        "tray_toggle",
        toggle_label,
        toggle_enabled,
        None::<&str>,
    )?;

    // ── 2. Focused polish actions ──────────────────────────────────────
    // Keep the visible tray menu intentionally small for senior/user-facing
    // builds. Existing Option+digit handlers remain wired for power users.
    #[cfg(target_os = "macos")]
    let (h_format, h_prof, h_hinglish) = ("Polish My Message  ⌥1", "English  ⌥2", "Hinglish  ⌥5");
    #[cfg(not(target_os = "macos"))]
    let (h_format, h_prof, h_hinglish) = ("Polish My Message", "English", "Hinglish");

    let p_format = MenuItem::with_id(
        app,
        "tray_polish_message_polish",
        h_format,
        true,
        None::<&str>,
    )?;
    let p_prof = MenuItem::with_id(app, "tray_polish_professional", h_prof, true, None::<&str>)?;
    let p_hinglish =
        MenuItem::with_id(app, "tray_polish_hinglish", h_hinglish, true, None::<&str>)?;

    // ── 3. Window actions + quit ────────────────────────────────────────
    let show_item = MenuItem::with_id(app, "show", "Open AirNote", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit AirNote", true, None::<&str>)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    Menu::with_items(
        app,
        &[
            &toggle as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
            &sep1,
            &p_format,
            &p_prof,
            &p_hinglish,
            &sep2,
            &show_item,
            &settings_item,
            &sep3,
            &quit_item,
        ],
    )
}

fn sync_status_bar(handle: &tauri::AppHandle, state: &str) {
    diag::breadcrumb(format!("status_bar:sync:{state}"));
    #[cfg(target_os = "macos")]
    {
        let app = handle.clone();
        let state = state.to_string();
        if let Err(e) = run_on_main_guarded(&app.clone(), "status_bar.sync", move || {
            sync_status_bar_on_main(&app, &state);
        }) {
            tracing::warn!("[status-bar] sync on main thread failed: {e}");
        }
        return;
    }
    #[cfg(not(target_os = "macos"))]
    sync_status_bar_on_main(handle, state);
}

fn sync_status_bar_on_main(handle: &tauri::AppHandle, state: &str) {
    #[cfg(target_os = "macos")]
    {
        hud_manager::MacHudManager::new(handle).sync_on_main(state);
        return;
    }

    #[cfg(not(target_os = "macos"))]
    {
        diag::breadcrumb(format!("status_bar:sync_main:{state}:begin"));
        let win = match handle.get_webview_window("status-bar") {
            Some(win) => win,
            None if state != "idle" => {
                diag::breadcrumb(format!("status_bar:sync_main:{state}:missing_recreate"));
                tracing::warn!(
                    "[status-bar] sync requested for active state={state}, but window was not found — recreating"
                );
                create_status_bar(handle);
                let Some(win) = handle.get_webview_window("status-bar") else {
                    diag::breadcrumb(format!("status_bar:sync_main:{state}:recreate_failed"));
                    tracing::warn!(
                        "[status-bar] recreate failed; still no status-bar window for state={state}"
                    );
                    return;
                };
                win
            }
            None => {
                diag::breadcrumb(format!("status_bar:sync_main:{state}:missing_idle"));
                tracing::warn!(
                    "[status-bar] sync requested for state={state}, but window was not found"
                );
                return;
            }
        };

        tracing::debug!("[status-bar] sync state={state}");
        if state == "idle" {
            if status_bar_pinned() || status_bar_persistent_hold(handle) {
                diag::breadcrumb("status_bar:sync_main:idle:held_visible");
                tracing::debug!("[status-bar] idle state — pinned/held, keeping visible");
                reposition_status_bar(handle, &win);
                let _ = win.set_always_on_top(true);
                let _ = win.set_visible_on_all_workspaces(true);
                let _ = win.show();
                return;
            }
            tracing::debug!("[status-bar] idle state — scheduling native hide");
            diag::breadcrumb("status_bar:sync_main:idle:schedule_hide");
            let my_gen = handle
                .try_state::<StatusBarHideGen>()
                .map(|s| s.0.fetch_add(1, Ordering::Relaxed) + 1)
                .unwrap_or(0);
            let hide_gen_arc = handle
                .try_state::<StatusBarHideGen>()
                .map(|s| Arc::clone(&s.0));
            let app = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(2600)).await;
                // If a newer idle sync fired after us, let it own the hide decision.
                if let Some(counter) = &hide_gen_arc {
                    if counter.load(Ordering::Relaxed) != my_gen {
                        return;
                    }
                }
                let still_idle = app
                    .try_state::<SharedApp>()
                    .and_then(|shared| {
                        shared
                            .0
                            .lock()
                            .ok()
                            .map(|d| d.state == desktop::AppState::Idle)
                    })
                    .unwrap_or(true);
                if !still_idle {
                    tracing::debug!("[status-bar] hide skipped — app is active again");
                    return;
                }
                if status_bar_pinned() || status_bar_persistent_hold(&app) {
                    tracing::debug!("[status-bar] hide skipped — status bar is pinned/held");
                    return;
                }
                if app.get_webview_window("status-bar").is_some() {
                    if let Some(win) = app.get_webview_window("status-bar") {
                        match win.hide() {
                            Ok(_) => tracing::debug!("[status-bar] hidden after idle"),
                            Err(e) => tracing::warn!("[status-bar] hide after idle failed: {e}"),
                        }
                    }
                }
            });
            return;
        }

        // Invalidate any pending idle-hide timer immediately. The timer also checks
        // state before hiding, but bumping the generation avoids relying on a mutex
        // read from an old async task when recordings happen in quick succession.
        if let Some(counter) = handle
            .try_state::<StatusBarHideGen>()
            .map(|s| Arc::clone(&s.0))
        {
            counter.fetch_add(1, Ordering::Relaxed);
        }

        diag::breadcrumb(format!("status_bar:sync_main:{state}:end"));

        reposition_status_bar(handle, &win);
        match win.set_always_on_top(true) {
            Ok(_) => tracing::debug!("[status-bar] set_always_on_top ok"),
            Err(e) => tracing::warn!("[status-bar] set_always_on_top failed: {e}"),
        }
        match win.set_visible_on_all_workspaces(true) {
            Ok(_) => tracing::debug!("[status-bar] set_visible_on_all_workspaces ok"),
            Err(e) => tracing::warn!("[status-bar] set_visible_on_all_workspaces failed: {e}"),
        }
        match win.show() {
            Ok(_) => tracing::debug!("[status-bar] show ok for state={state}"),
            Err(e) => tracing::warn!("[status-bar] show failed for state={state}: {e}"),
        }
        if state != "placement" {
            emit_status_bar_resync(handle, state);
        }
    }
}

/// Re-render the tray icon title + menu from the cached prefs (no async needed).
fn sync_tray(handle: &tauri::AppHandle, snap: &AppSnapshot) {
    #[cfg(target_os = "macos")]
    {
        let app = handle.clone();
        let state = snap.state.clone();
        if let Err(e) = run_on_main_guarded(&app.clone(), "tray.sync", move || {
            sync_tray_on_main(&app, &state);
        }) {
            tracing::warn!("[tray] sync on main thread failed: {e}");
        }
        return;
    }
    #[cfg(not(target_os = "macos"))]
    sync_tray_on_main(handle, &snap.state);
}

fn sync_tray_on_main(handle: &tauri::AppHandle, state: &str) {
    if let Some(tray) = handle.tray_by_id("said") {
        let _ = tray.set_title(Some(tray_title(state)));

        // Read from in-process cache — never blocks on async or HTTP
        let cache = handle.state::<TrayCache>();
        let inner = cache.0.lock().unwrap_or_else(|p| p.into_inner());
        let custom = inner.custom_prompt.clone();
        let lang = inner.output_language.clone();
        drop(inner);

        let _ = (custom, lang);
        if let Ok(menu) = build_tray_menu_for_state(handle, state) {
            let _ = tray.set_menu(Some(menu));
        }
    }

    #[cfg(target_os = "macos")]
    sync_status_bar_on_main(handle, state);
    #[cfg(not(target_os = "macos"))]
    sync_status_bar(handle, state);
}

// ── Floating status bar ───────────────────────────────────────────────────────

/// When true, the native notch HUD sidecar owns the dictation surface and the
/// legacy status-bar pill is fully suppressed. Set once at startup. Guarding
/// `create_status_bar` here is the single chokepoint — it also neutralises the
/// auto-recreate paths in `present_status_bar_native` / `sync_status_bar_on_main`.
static NOTCH_FIRST_CLASS: AtomicBool = AtomicBool::new(false);

/// Create the always-on-top floating status pill.
///
/// The window loads the same SPA with an explicit statusbar marker so
/// `main.tsx` renders `<StatusBar />` instead of the full app. It starts hidden
/// at idle and is shown by `sync_status_bar()` when recording/processing begins.
pub(crate) fn create_status_bar(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        hud_manager::MacHudManager::new(app).ensure_created();
        return;
    }

    #[cfg(not(target_os = "macos"))]
    {
        if NOTCH_FIRST_CLASS.load(Ordering::Relaxed) {
            // Notch sidecar is the HUD; never bring up the pill (incl. auto-recreate).
            return;
        }
        if app.get_webview_window("status-bar").is_some() {
            tracing::info!("[status-bar] create skipped; window already exists");
            return;
        }

        let idle_w = STATUS_BAR_WIDTH;
        let idle_h = STATUS_BAR_HEIGHT;
        let (x, y) = status_bar_target_origin(app, idle_w, idle_h);

        let recovery_preview_enabled = std::env::var("AIRNOTE_RECOVERY_PREVIEW")
            .or_else(|_| std::env::var("VITE_AIRNOTE_RECOVERY_PREVIEW"))
            .map(|v| v == "1")
            .unwrap_or(false);
        let url = if recovery_preview_enabled {
            "index.html?view=statusbar&recoveryPreview=1#statusbar"
        } else {
            "index.html?view=statusbar#statusbar"
        };
        tracing::info!(
            "[status-bar] creating window url={url} x={x:.0} y={y:.0} size={idle_w:.0}x{idle_h:.0} visible=false"
        );

        match tauri::WebviewWindowBuilder::new(
            app,
            "status-bar",
            tauri::WebviewUrl::App(url.into()),
        )
        .title("AirNote")
        .background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .inner_size(idle_w, idle_h)
        .position(x, y)
        .decorations(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .focused(false)
        .resizable(true)
        .shadow(false)
        .transparent(true)
        .visible(false)
        .build()
        {
            Ok(win) => {
                tracing::info!("[status-bar] window created label={}", win.label());
                match win.url() {
                    Ok(url) => tracing::info!("[status-bar] resolved url={url}"),
                    Err(e) => tracing::warn!("[status-bar] could not read window url: {e}"),
                }
                let _ = win.set_ignore_cursor_events(true);
                sync_status_bar(app, "idle");
            }
            Err(e) => tracing::warn!("[status-bar] could not create window: {e}"),
        }
    }
}

#[cfg(target_os = "macos")]
fn schedule_status_bar_startup_preload(app: &tauri::AppHandle) {
    let app_h = app.clone();
    tauri::async_runtime::spawn(async move {
        // Avoid creating AppKit/WebView state inside Tauri setup, but have the
        // hidden panel ready before the first hold-to-talk transition.
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
        let app_main = app_h.clone();
        if let Err(e) = run_on_main_guarded(&app_h, "status_bar.startup_preload", move || {
            if app_main.get_webview_window("status-bar").is_some() {
                return;
            }
            diag::breadcrumb("status_bar:startup_preload:create");
            tracing::info!("[status-bar] startup preload creating hidden HUD");
            create_status_bar(&app_main);
        }) {
            tracing::warn!("[status-bar] startup preload failed: {e}");
        }
    });
}

// ── Tray action helpers ───────────────────────────────────────────────────────

fn emit_tray_error(app: &tauri::AppHandle, message: impl Into<String>) {
    let _ = app.emit(
        "voice-error",
        serde_json::json!({
            "message": message.into(),
            "audio_id": null,
        }),
    );
}

fn show_main_window(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        activate_airnote(app, "show_main_window");
        return;
    }

    #[cfg(not(target_os = "macos"))]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

// ── Live-meeting floating pill ────────────────────────────────────────────────
// A tiny capsule shown while a meeting records and the app isn't in front, so
// recording status stays visible over ANYTHING — including other apps' full
// screen Spaces. macOS uses a true NSPanel (the only way to float over another
// app's full-screen Space); other platforms use an always-on-top window.

// The capsule fills the window exactly. No drop shadow/glow (those clipped into
// grey "corners" on the transparent window) — the window's corners outside the
// pill's rounded ends are fully transparent.
const MEETING_PILL_W: f64 = 200.0;
const MEETING_PILL_H: f64 = 52.0;

/// Force the pill's native window fully transparent (clear background, not
/// opaque, no shadow) so only the rounded capsule shows — no square backing.
#[cfg(target_os = "macos")]
fn make_pill_transparent_macos(app: &tauri::AppHandle) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    let Some(win) = app.get_webview_window("meeting-pill") else {
        return;
    };
    let Ok(ns_window) = win.ns_window() else {
        return;
    };
    if ns_window.is_null() {
        return;
    }
    unsafe {
        let ns_window = ns_window as *mut Object;
        let clear: *mut Object = msg_send![class!(NSColor), clearColor];
        let _: () = msg_send![ns_window, setOpaque: false];
        let _: () = msg_send![ns_window, setBackgroundColor: clear];
        let _: () = msg_send![ns_window, setHasShadow: false];
    }
}

fn meeting_pill_position(app: &tauri::AppHandle) -> (f64, f64) {
    app.get_webview_window("main")
        .and_then(|w| w.current_monitor().ok().flatten())
        .map(|m| {
            let logical_w = m.size().width as f64 / m.scale_factor();
            (((logical_w - MEETING_PILL_W) / 2.0).max(8.0), 16.0)
        })
        .unwrap_or((620.0, 16.0))
}

#[tauri::command]
fn show_meeting_pill(app: tauri::AppHandle) {
    let (x, y) = meeting_pill_position(&app);

    #[cfg(target_os = "macos")]
    {
        let url = "index.html?view=meeting-pill#meeting-pill";
        let app_for_main = app.clone();
        if let Err(e) = run_on_main_guarded(&app, "meeting_pill.show", move || {
            if let Ok(panel) = app_for_main.get_webview_panel("meeting-pill") {
                panel.show();
                panel.order_front_regardless();
                return;
            }
            match PanelBuilder::<_, MeetingPillPanel>::new(&app_for_main, "meeting-pill")
                .url(tauri::WebviewUrl::App(url.into()))
                .title("AirNote Meeting")
                .size(tauri::Size::Logical(tauri::LogicalSize::new(
                    MEETING_PILL_W,
                    MEETING_PILL_H,
                )))
                .position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
                .level(PanelLevel::Custom(28))
                .floating(true)
                .hides_on_deactivate(false)
                .works_when_modal(true)
                .ignores_mouse_events(false)
                .has_shadow(false)
                .transparent(true)
                .style_mask(StyleMask::empty().borderless().nonactivating_panel())
                .collection_behavior(
                    CollectionBehavior::new()
                        .can_join_all_spaces()
                        .full_screen_auxiliary(),
                )
                .no_activate(true)
                .with_window(|window| {
                    window
                        .decorations(false)
                        .always_on_top(true)
                        .visible_on_all_workspaces(true)
                        .skip_taskbar(true)
                        .focused(false)
                        .resizable(false)
                        .shadow(false)
                        .transparent(true)
                })
                .build()
            {
                Ok(panel) => {
                    panel.show();
                    panel.order_front_regardless();
                    make_pill_transparent_macos(&app_for_main);
                    tracing::info!("[meeting-pill] NSPanel created");
                }
                Err(e) => tracing::warn!("[meeting-pill] panel create failed: {e}"),
            }
            make_pill_transparent_macos(&app_for_main);
        }) {
            tracing::warn!("[meeting-pill] schedule show on main thread failed: {e}");
        }
        return;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // The pill window is pre-created (hidden) at startup — see setup(). NEVER
        // create it here: building a WebView2 window from inside this in-flight IPC
        // command deadlocks Windows IPC (wry #583) and wedges End-meeting. We only
        // reposition + show the existing window.
        if let Some(w) = app.get_webview_window("meeting-pill") {
            let _ = w.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)));
            let _ = w.show();
            let _ = w.set_always_on_top(true);
        } else {
            tracing::warn!("[meeting-pill] window not pre-created; skipping show");
        }
    }
}

#[tauri::command]
fn hide_meeting_pill(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    {
        let app_for_main = app.clone();
        if let Err(e) = run_on_main_guarded(&app, "meeting_pill.hide", move || {
            if let Ok(panel) = app_for_main.get_webview_panel("meeting-pill") {
                panel.hide();
                return;
            }
            if let Some(w) = app_for_main.get_webview_window("meeting-pill") {
                let _ = w.hide();
            }
        }) {
            tracing::warn!("[meeting-pill] schedule hide on main thread failed: {e}");
        }
        return;
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(w) = app.get_webview_window("meeting-pill") {
            let _ = w.hide();
        }
    }
}

#[tauri::command]
fn focus_main_from_pill(app: tauri::AppHandle) {
    show_main_window(&app);
    hide_meeting_pill(app);
}

fn launched_from_autostart() -> bool {
    std::env::args().any(|arg| arg == AUTOSTART_ARG)
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn activate_airnote(app: &tauri::AppHandle, reason: &'static str) {
    let app_h = app.clone();
    let _ = run_on_main_guarded(app, "activate_airnote", move || {
        use cocoa::appkit::{NSApp, NSApplication};
        use cocoa::base::YES;

        unsafe {
            NSApp().activateIgnoringOtherApps_(YES);
        }
        if let Some(w) = app_h.get_webview_window("main") {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        tracing::debug!("[main] activated AirNote window ({reason})");
    });
}

#[cfg(not(target_os = "macos"))]
fn activate_airnote(_app: &tauri::AppHandle, _reason: &'static str) {}

#[cfg(target_os = "macos")]
fn activate_airnote_after(app: &tauri::AppHandle, reason: &'static str, delay_ms: u64) {
    let app_h = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        activate_airnote(&app_h, reason);
    });
}

#[cfg(not(target_os = "macos"))]
fn activate_airnote_after(_app: &tauri::AppHandle, _reason: &'static str, _delay_ms: u64) {}

fn apply_launch_at_login(app: &tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app
        .try_state::<tauri_plugin_autostart::AutoLaunchManager>()
        .ok_or_else(|| "autostart manager unavailable".to_string())?;
    let current = manager
        .is_enabled()
        .map_err(|e| format!("read launch-at-login state: {e}"))?;
    if enabled == current {
        return Ok(());
    }
    if enabled {
        manager
            .enable()
            .map_err(|e| format!("enable launch-at-login: {e}"))?;
    } else {
        manager
            .disable()
            .map_err(|e| format!("disable launch-at-login: {e}"))?;
    }
    tracing::info!("[prefs] launch_at_login applied: {enabled}");
    Ok(())
}

fn activate_long_dictation_lock(app: &tauri::AppHandle) {
    let Some(long) = app.try_state::<LongDictationState>() else {
        return;
    };
    long.pending_lock.store(false, Ordering::SeqCst);
    if !long.locked.swap(true, Ordering::SeqCst) {
        tracing::info!("[hotkey] long dictation locked");
        let _ = app.emit("long-dictation-locked", serde_json::json!({}));
    }
}

fn reset_long_dictation_lock(app: &tauri::AppHandle) {
    if let Some(long) = app.try_state::<LongDictationState>() {
        if long.reset() {
            let _ = app.emit("long-dictation-unlocked", serde_json::json!({}));
        }
    }
}

fn clear_long_dictation_recording_lock(app: &tauri::AppHandle) {
    if let Some(long) = app.try_state::<LongDictationState>() {
        if long.clear_recording_lock() {
            let _ = app.emit("long-dictation-unlocked", serde_json::json!({}));
        }
    }
}

fn clear_long_dictation_finishing(app: &tauri::AppHandle, reason: &'static str) {
    if let Some(long) = app.try_state::<LongDictationState>() {
        if long.clear_finishing() {
            tracing::debug!("[hotkey] long dictation finishing cleared ({reason})");
        }
    }
}

fn long_dictation_finishing(app: &tauri::AppHandle) -> bool {
    app.try_state::<LongDictationState>()
        .map(|long| long.finishing.load(Ordering::SeqCst))
        .unwrap_or(false)
}

fn stop_long_dictation_lock(
    app: &tauri::AppHandle,
    locked: &AtomicBool,
    stop_consumed: &AtomicBool,
    finishing: &AtomicBool,
) {
    stop_consumed.store(true, Ordering::SeqCst);
    finishing.store(true, Ordering::SeqCst);
    if locked.swap(false, Ordering::SeqCst) {
        tracing::info!("[hotkey] long dictation unlocked for processing");
        let _ = app.emit("long-dictation-unlocked", serde_json::json!({}));
    }
}

fn show_status_bar_placement_mode(app: &tauri::AppHandle, message: &'static str) {
    if app.get_webview_window("status-bar").is_none() {
        create_status_bar(app);
    }
    set_placement_mode_active(app, true);
    sync_status_bar(app, "placement");
    let _ = app.emit(
        "status-bar-placement-mode",
        serde_json::json!({ "message": message }),
    );
}

fn reset_status_bar_to_default(app: &tauri::AppHandle) {
    if let Err(e) = reset_status_bar_position(app.clone()) {
        tracing::warn!("[status-bar] reset shortcut failed: {e}");
    }
}

fn finish_status_bar_placement_mode(app: &tauri::AppHandle) {
    set_placement_mode_active(app, false);
    let _ = app.emit("status-bar-placement-finish", serde_json::json!({}));
    if let Err(e) = dismiss_status_bar(app.clone()) {
        tracing::warn!("[status-bar] finish placement hide failed: {e}");
    }
}

fn toggle_status_bar_placement_mode(app: &tauri::AppHandle) {
    if placement_mode_active(app) {
        tracing::info!("[status-bar] ⇧⌘/ — finish placement mode");
        finish_status_bar_placement_mode(app);
    } else {
        tracing::info!("[status-bar] ⇧⌘/ — enter placement mode");
        show_status_bar_placement_mode(app, "Drag AirNote");
    }
}

/// Trigger recording from a tray menu click.
/// Mirrors the `toggle_recording` Tauri command's logic.
fn tray_toggle_recording(app: &tauri::AppHandle) {
    let shared_state = app.state::<SharedApp>();
    let backend_state = app.state::<BackendState>();

    let current = match shared_state.0.lock() {
        Ok(g) => g.state,
        Err(_) => return,
    };

    match current {
        desktop::AppState::Idle => {
            do_start_recording(&shared_state.0, app);
        }
        desktop::AppState::Recording => {
            do_finish_recording(
                Arc::clone(&shared_state.0),
                app.clone(),
                Arc::clone(&backend_state.0),
            );
        }
        desktop::AppState::Processing => {} // ignore — already in flight
    }
}

fn toggle_message_polish_mode(app: &tauri::AppHandle) {
    let mut prefs = said_core::prefs::load();
    prefs.message_polish_mode = !prefs.message_polish_mode;
    if let Err(e) = said_core::prefs::save(&prefs) {
        tracing::warn!("[message_polish] failed to persist mode toggle: {e}");
        emit_tray_error(app, "Couldn't save polish mode. Try again.");
        return;
    }

    tracing::info!(
        "[message_polish] mode toggled {}",
        if prefs.message_polish_mode {
            "on"
        } else {
            "off"
        }
    );
    let _ = present_status_bar_native(app, "message-polish-mode", false);
    let _ = app.emit(
        "message-polish-mode",
        serde_json::json!({
            "enabled": prefs.message_polish_mode,
            "message": if prefs.message_polish_mode { "Polish mode on" } else { "Polish mode off" },
        }),
    );
}

/// Insert dictation output into the focused field. Delegates the direct-first /
/// clipboard-fallback policy to `paster::insert_text` (one source of truth). The
/// returned bool is `used_clipboard`, which the edit-watcher uses to decide how
/// to reconcile later.
fn insert_text_prefer_direct(label: &str, text: &str) -> (Result<(), String>, bool) {
    match paster::insert_text(text) {
        Ok(paster::InsertMethod::Typed) => (Ok(()), false),
        Ok(paster::InsertMethod::Clipboard) => (Ok(()), true),
        Err(e) => {
            tracing::warn!("[{label}] text insert failed: {e}");
            (Err(e), true)
        }
    }
}

/// Polish the currently selected text using the given tone preset.
///
/// Flow: read selection → POST /v1/text/polish (SSE) with tone_override → paste result.
fn tray_polish_message(app: &tauri::AppHandle, tone: &str) {
    cancel_edit_watcher(app, "tray polish (Option+1) triggered");
    let backend = app.state::<BackendState>();
    let ep_opt = backend.0.lock().ok().and_then(|g| g.clone());
    let ep = match ep_opt {
        Some(e) => e,
        None => {
            tracing::warn!("[tray_polish] backend not ready");
            emit_tray_error(
                app,
                "AirNote backend is still starting. Try again in a moment.",
            );
            return;
        }
    };

    // Read the selected text.  This is called from a spawned thread (not the
    // CGEventTap thread) so the Cmd+C fallback can work.
    tracing::info!("[tray_polish] reading selected text for tone={tone}...");
    let selected = paster::read_selected_text();
    let text = match selected {
        Some(t) if !t.trim().is_empty() => {
            tracing::info!("[tray_polish] got {} chars of selected text", t.len());
            t
        }
        _ => {
            tracing::warn!(
                "[tray_polish] no text selected — make sure text is highlighted before pressing Option+N"
            );
            emit_tray_error(
                app,
                "Select text first, then choose a Polish my message action.",
            );
            return;
        }
    };

    let tone_owned = tone.to_string();
    let app_clone = app.clone();
    let source_text = text.clone();

    tauri::async_runtime::spawn(async move {
        tracing::info!(
            "[tray_polish] polishing {} chars with tone={}",
            text.len(),
            tone_owned
        );

        let result = api::stream_text_polish(
            &ep,
            text,
            None,
            Some(tone_owned.clone()),
            |_event| {}, // fire-and-forget on events; we paste the final result
        )
        .await;

        match result {
            Ok(done) if !done.polished.is_empty() => {
                tracing::info!(
                    "[tray_polish] done — {} words",
                    done.polished.split_whitespace().count()
                );
                // Emit tokens to the UI for live preview if the window is visible
                emit_voice_done(&app_clone, &done, None);
                // Selected-text transform must reliably replace the active
                // selection. Direct Unicode typing has no success signal from
                // macOS, so this explicit command keeps the proven paste path.
                if let Err(e) = paster::paste(&done.polished) {
                    tracing::warn!("[tray_polish] paste failed: {e}");
                }
                if let Ok(mut g) = app_clone.state::<LatestResult>().0.lock() {
                    *g = Some(done.polished.clone());
                }
                cache_last_text_transform(
                    &app_clone,
                    source_text,
                    done.polished.clone(),
                    tone_owned.clone(),
                );
            }
            Ok(_) => {
                tracing::warn!("[tray_polish] empty result from backend");
                emit_tray_error(&app_clone, "Polish returned no text. Try again.");
            }
            Err(e) => {
                let human = humanize_error(&e);
                tracing::warn!("[tray_polish] backend error: {e}");
                emit_tray_error(&app_clone, human);
            }
        }
    });
}

/// Show the main window and emit a hint to switch to the settings view.
fn tray_open_settings(app: &tauri::AppHandle) {
    show_main_window(app);
    let _ = app.emit("nav-settings", ());
}

fn smart_repair_last(app: &tauri::AppHandle) {
    let last_action = app
        .state::<LastActionState>()
        .0
        .lock()
        .ok()
        .and_then(|g| g.clone());
    let Some(last_action) = last_action else {
        let _ = app.emit(
            "voice-error",
            serde_json::json!({
                "message": "Nothing recent to repair yet.",
                "audio_id": null,
            }),
        );
        return;
    };

    match last_action {
        LastAction::Voice(action) => {
            let now = now_ms();
            let should_escalate = matches!(action.last_repair_stage, LastRepairStage::FastRepair)
                && action
                    .last_repair_at_ms
                    .map(|t| now.saturating_sub(t) <= 20_000)
                    .unwrap_or(false);

            if should_escalate {
                if let Some(audio_id) = action.audio_id.clone() {
                    retry_recording_internal(app.clone(), audio_id);
                } else {
                    let _ = app.emit(
                        "voice-error",
                        serde_json::json!({
                            "message": "Saved audio is no longer available for full reprocess.",
                            "audio_id": null,
                        }),
                    );
                }
            } else {
                run_fast_voice_repair(app.clone(), action);
            }
        }
        LastAction::TextTransform(action) => {
            run_refine_last_transform(app.clone(), action);
        }
    }
}

/// ⌃⌥R: re-run the LAST dictation as a fresh, FULL-quality attempt — re-transcribe
/// (STT) AND re-polish from the saved audio. Unlike `smart_repair_last` (which tries
/// a quick re-polish first and only escalates to a full reprocess on a second press),
/// this ALWAYS does the full reprocess from the recorded voice, so the user gets a
/// proper attempt even if the first transcript or polish failed — never a degraded one.
fn retry_last_from_audio(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let endpoint = app
            .try_state::<BackendState>()
            .and_then(|state| state.0.lock().ok()?.clone());
        if let Some(ep) = endpoint {
            match api::latest_failed_voice_run(&ep).await {
                Ok(Some(run)) => {
                    if let Some(audio_id) = run
                        .audio_id
                        .clone()
                        .filter(|id| saved_voice_audio_exists(id))
                    {
                        let message_polish = run.mode == "message_polish";
                        tracing::info!(
                            "[retry] DB-first latest failed run run_id={} audio_id={} mode={} attempts={} error_code={}",
                            run.run_id,
                            audio_id,
                            run.mode,
                            run.attempt_count,
                            run.error_code.as_deref().unwrap_or("none"),
                        );
                        retry_recording_internal_with_mode(
                            app.clone(),
                            audio_id,
                            Some(message_polish),
                            "latest_failed_db",
                        );
                        return;
                    }
                    tracing::warn!(
                        "[retry] DB latest failed run has no available audio run_id={} audio_id={}",
                        run.run_id,
                        run.audio_id.as_deref().unwrap_or("none"),
                    );
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("[retry] latest failed run lookup failed: {e}"),
            }
        }
        retry_last_from_audio_fallback(&app);
    });
}

fn retry_last_from_audio_fallback(app: &tauri::AppHandle) {
    if let Some(failed) = app
        .try_state::<LastFailedVoiceActionState>()
        .and_then(|state| state.0.lock().ok()?.clone())
        .filter(|action| saved_voice_audio_exists(&action.audio_id))
    {
        tracing::info!(
            "[retry] full reprocess from last failed audio audio_id={} message_polish={} age_ms={} error_code={} raw_error={}",
            failed.audio_id,
            failed.message_polish_mode,
            now_ms().saturating_sub(failed.failed_at_ms),
            failed.error_code.as_deref().unwrap_or("none"),
            failed.raw_error,
        );
        retry_recording_internal_with_mode(
            app.clone(),
            failed.audio_id,
            Some(failed.message_polish_mode),
            "last_failed",
        );
        return;
    }

    let last_action = app
        .state::<LastActionState>()
        .0
        .lock()
        .ok()
        .and_then(|g| g.clone());
    match last_action {
        Some(LastAction::Voice(action)) => match action.audio_id.clone() {
            Some(audio_id) => {
                tracing::info!(
                    "[retry] full reprocess from last successful saved audio {audio_id}"
                );
                retry_recording_internal_with_mode(app.clone(), audio_id, None, "last_success");
            }
            None => {
                let _ = app.emit(
                    "voice-error",
                    serde_json::json!({
                        "message": "Saved audio is no longer available to retry.",
                        "audio_id": null,
                    }),
                );
            }
        },
        _ => {
            if let Some(audio_id) = latest_saved_voice_audio_id() {
                tracing::info!(
                    "[retry] no cached action; falling back to newest saved audio {audio_id}"
                );
                retry_recording_internal_with_mode(app.clone(), audio_id, None, "newest_wav");
            } else {
                let _ = app.emit(
                    "voice-error",
                    serde_json::json!({
                        "message": "Nothing recent to retry yet.",
                        "audio_id": null,
                    }),
                );
            }
        }
    }
}

/// Switch output language from the tray menu and persist to SQLite.
fn tray_set_output_language(app: &tauri::AppHandle, lang: &str) {
    if !matches!(lang, "hinglish" | "english" | "hindi") {
        tracing::warn!("[tray_lang] ignored unknown output language: {lang}");
        return;
    }

    // Update cache immediately so sync_tray shows the new checkmark
    if let Ok(mut cache) = app.state::<TrayCache>().0.lock() {
        cache.output_language = lang.to_string();
    }
    // Re-render tray with new checkmark
    let shared = app.state::<SharedApp>();
    if let Ok(d) = shared.0.lock() {
        let snap = d.snapshot();
        drop(d);
        sync_tray(app, &snap);
    }
    // Persist to backend (fire-and-forget)
    let backend = app.state::<BackendState>();
    let ep_opt = backend.0.lock().ok().and_then(|g| g.clone());
    let lang_own = lang.to_string();
    let app_h = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(ep) = ep_opt else {
            emit_tray_error(
                &app_h,
                "AirNote backend is still starting. Language will update once Settings are available.",
            );
            return;
        };

        match api::patch_preferences(
            &ep,
            api::PrefsUpdate {
                output_language: Some(lang_own),
                ..Default::default()
            },
        )
        .await
        {
            Ok(_) => {
                // Tell the frontend to refresh its prefs so the settings page stays in sync
                let _ = app_h.emit("prefs-changed", ());
            }
            Err(e) => {
                tracing::warn!("[tray_lang] failed to persist output language: {e}");
                emit_tray_error(&app_h, "Could not save output language. Try Settings.");
            }
        }
    });
}

// ── Tauri commands ────────────────────────────────────────────────────────────

#[tauri::command]
fn bootstrap(state: State<'_, SharedApp>, app: tauri::AppHandle) -> Result<AppSnapshot, String> {
    let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
    sync_tray(&app, &snap);
    Ok(snapshot_for_ui(&app, &snap))
}

// SYNC (deliberately): a sync Tauri command returns its response synchronously in
// the ipc:// protocol handler, so the request completes immediately and frees its
// WebView2 connection. An `async` version defers the response, and on Windows that
// deferred delivery piled up Pending — hundreds of un-drained `get_snapshot`
// requests saturated the ~6-connection pool so NO new invoke (incl. End-meeting's
// stop_session) could dispatch. Keep this sync; it's a quick mutex read.
#[tauri::command]
fn get_snapshot(state: State<'_, SharedApp>, app: tauri::AppHandle) -> Result<AppSnapshot, String> {
    let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
    Ok(snapshot_for_ui(&app, &snap))
}

/// Fault injection for resilience testing. Inert unless `AIRNOTE_CHAOS=1` is set
/// at launch. `kind` is one of: main_panic | pipeline_panic | stick_processing |
/// plant_orphan | drop_hud | emit_diag. Returns a short status string.
#[tauri::command]
fn chaos_inject(app: tauri::AppHandle, kind: String) -> Result<String, String> {
    Ok(chaos::inject(&app, &kind))
}

#[tauri::command]
fn dismiss_status_bar(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app_c = app.clone();
        if let Err(e) = run_on_main_guarded(&app, "status_bar.dismiss", move || {
            if let Err(e) = dismiss_status_bar_on_main(&app_c) {
                tracing::warn!("[status-bar] dismiss failed: {e}");
            }
        }) {
            return Err(format!("schedule dismiss failed: {e}"));
        }
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        dismiss_status_bar_on_main(&app)
    }
}

fn dismiss_status_bar_on_main(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return hud_manager::MacHudManager::new(app).dismiss_on_main();
    }

    #[cfg(not(target_os = "macos"))]
    {
        if status_bar_pinned() || status_bar_persistent_hold(app) {
            return Ok(());
        }
        let is_active = app
            .try_state::<SharedApp>()
            .and_then(|shared| {
                shared
                    .0
                    .lock()
                    .ok()
                    .map(|d| d.state != desktop::AppState::Idle)
            })
            .unwrap_or(false);
        if is_active {
            tracing::debug!("[status-bar] dismiss skipped — app state is active");
            return Ok(());
        }
        if let Some(win) = app.get_webview_window("status-bar") {
            win.hide()
                .map_err(|e| format!("hide status bar failed: {e}"))?;
        }
        Ok(())
    }
}

#[tauri::command]
fn present_status_bar(app: tauri::AppHandle, reason: Option<String>) -> Result<(), String> {
    let reason = reason.unwrap_or_else(|| "notification".to_string());
    tracing::debug!("[status-bar] present requested reason={reason}");
    present_status_bar_native(&app, &reason, false)
}

fn start_runtime_notification_listener(app: tauri::AppHandle, ep: BackendEndpoint) {
    tauri::async_runtime::spawn(async move {
        let mut retry_delay = Duration::from_secs(5);
        loop {
            let config = match api::get_runtime_notification_config(&ep).await {
                Ok(config) => config,
                Err(e) => {
                    tracing::debug!("[runtime-notify] notification config unavailable: {e}");
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    continue;
                }
            };
            let Some(ws_url) = config.notifications_ws_url else {
                tracing::debug!(
                    "[runtime-notify] notification config missing ws_url (no cloud token?)"
                );
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            };
            tracing::info!(
                "[runtime-notify] notification config loaded; connecting to server notifications WS"
            );

            let socket = match connect_async(&ws_url).await {
                Ok((socket, _)) => socket,
                Err(e) => {
                    tracing::warn!("[runtime-notify] WS connect failed: {e}");
                    tokio::time::sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    continue;
                }
            };
            retry_delay = Duration::from_secs(5);
            tracing::info!("[runtime-notify] notification WS connected");
            let (mut sink, mut stream) = socket.split();
            let mut heartbeat = tokio::time::interval(Duration::from_secs(30));

            loop {
                tokio::select! {
                    _ = heartbeat.tick() => {
                        let _ = sink.send(WsMessage::Text(serde_json::json!({"type":"ping"}).to_string())).await;
                    }
                    msg = stream.next() => {
                        let Some(msg) = msg else { break };
                        let Ok(msg) = msg else { break };
                        let WsMessage::Text(text) = msg else { continue };
                        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                            continue;
                        };
                        let Some(event_name) = value.get("type").and_then(serde_json::Value::as_str) else {
                            continue;
                        };
                        if matches!(event_name, "notification.connected" | "pong") {
                            tracing::debug!("[runtime-notify] server event {event_name}");
                            continue;
                        }
                        if !is_status_bar_notification_event(event_name) {
                            tracing::info!("[runtime-notify] ignored server event {event_name}");
                            continue;
                        }
                        let payload = value
                            .get("payload")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        tracing::info!("[runtime-notify] forwarding event {event_name} to status bar");
                        let _ = present_status_bar_native(&app, event_name, false);
                        if let Err(e) = app.emit(event_name, payload) {
                            tracing::warn!("[runtime-notify] emit {event_name} failed: {e}");
                        }
                    }
                }
            }

            tracing::warn!("[runtime-notify] notification WS disconnected; will reconnect");
            tokio::time::sleep(retry_delay).await;
        }
    });
}

fn is_status_bar_notification_event(event_name: &str) -> bool {
    matches!(
        event_name,
        "vocab-learned"
            | "email-learned"
            | "vocab-queued"
            | "vocab-confirm"
            | "vocab-review"
            | "vocab-negative"
            | "vocab-wrong-fixed"
            | "retrain-status"
            | "auto-update-ready"
            | "voice-error"
            | "message-polish-mode"
            | "learning_saved"
    )
}

#[tauri::command]
fn set_status_bar_persistent(
    app: tauri::AppHandle,
    persistent: bool,
    reason: Option<String>,
    interactive: Option<bool>,
) -> Result<(), String> {
    let reason = reason.unwrap_or_else(|| "notification".to_string());
    let hold = app
        .try_state::<StatusBarPersistentHold>()
        .ok_or_else(|| "status-bar persistent hold state missing".to_string())?;
    hold.0.store(persistent, Ordering::SeqCst);
    tracing::debug!("[status-bar] persistent_hold={persistent} reason={reason}");
    if persistent {
        // The hold keeps the panel visible past idle syncs. Interactivity is a
        // separate concern set by the caller: actionable prompts (updates, the
        // Divo answer) want clicks; a passive run (Divo streaming) stays
        // click-through so it never steals a click from the user's app. Apply it
        // from Rust immediately rather than relying on a later React effect.
        set_status_bar_interactive_state(&app, interactive.unwrap_or(true));
        present_status_bar_native(&app, &reason, false)?;
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn read_window_bottom_anchor(win: &tauri::WebviewWindow) -> Option<(f64, f64)> {
    let scale = win.scale_factor().ok()?;
    let pos = win.outer_position().ok()?;
    let size = win.inner_size().ok()?;
    let w = size.width as f64 / scale;
    let h = size.height as f64 / scale;
    if w < 1.0 || h < 1.0 {
        return None;
    }
    let x = pos.x as f64 / scale;
    let y = pos.y as f64 / scale;
    Some((x + w / 2.0, y + h))
}

#[cfg(not(target_os = "macos"))]
fn origin_from_bottom_anchor(center_x: f64, bottom_y: f64, width: f64, height: f64) -> (f64, f64) {
    (center_x - width / 2.0, bottom_y - height)
}

#[tauri::command]
fn resize_status_bar(app: tauri::AppHandle, width: f64, height: f64) -> Result<(), String> {
    // Always marshal window ops onto the main thread (was macOS-only). Doing
    // set_size/set_position inline inside this IPC handler on Windows can stall the
    // webview (tao #381 — window ops from a command freeze on Windows), and the
    // status-bar fires resize on every pill size/state change.
    let app_c = app.clone();
    if let Err(e) = run_on_main_guarded(&app, "status_bar.resize", move || {
        if let Err(e) = resize_status_bar_on_main(&app_c, width, height) {
            tracing::warn!("[status-bar] resize failed: {e}");
        }
    }) {
        return Err(format!("schedule resize failed: {e}"));
    }
    Ok(())
}

fn resize_status_bar_on_main(
    app: &tauri::AppHandle,
    width: f64,
    height: f64,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return hud_manager::MacHudManager::new(app).resize_on_main(width, height);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let win = app
            .get_webview_window("status-bar")
            .ok_or_else(|| "status-bar window not found".to_string())?;
        let (center_x, bottom_y) = read_window_bottom_anchor(&win).unwrap_or_else(|| {
            let (x, y) = status_bar_target_origin(app, width, height);
            (x + width / 2.0, y + height)
        });
        win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)))
            .map_err(|e| format!("resize status bar failed: {e}"))?;
        let (x, y) = origin_from_bottom_anchor(center_x, bottom_y, width, height);
        win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
            .map_err(|e| format!("post-resize position failed: {e}"))?;
        Ok(())
    }
}

#[tauri::command]
fn get_status_bar_position(app: tauri::AppHandle) -> Result<Option<serde_json::Value>, String> {
    let Some(anchor) = load_status_bar_anchor() else {
        return Ok(None);
    };
    let Some(win) = app.get_webview_window("status-bar") else {
        return Ok(None);
    };
    let scale = win.scale_factor().unwrap_or(1.0);
    let size = win.inner_size().map_err(|e| format!("inner_size: {e}"))?;
    let w = size.width as f64 / scale;
    let h = size.height as f64 / scale;
    let (x, y) = status_bar_origin_from_anchor(anchor, w, h);
    Ok(Some(serde_json::json!({ "x": x, "y": y })))
}

#[tauri::command]
fn set_status_bar_position(app: tauri::AppHandle, x: f64, y: f64) -> Result<(), String> {
    // Marshal onto the main thread on every platform (see resize_status_bar).
    let app_c = app.clone();
    if let Err(e) = run_on_main_guarded(&app, "status_bar.set_position", move || {
        if let Err(e) = set_status_bar_position_on_main(&app_c, x, y) {
            tracing::warn!("[status-bar] set position failed: {e}");
        }
    }) {
        return Err(format!("schedule set position failed: {e}"));
    }
    Ok(())
}

fn set_status_bar_position_on_main(app: &tauri::AppHandle, x: f64, y: f64) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return hud_manager::MacHudManager::new(app).set_position_on_main(x, y);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let win = app
            .get_webview_window("status-bar")
            .ok_or_else(|| "status-bar window not found".to_string())?;
        let scale = win.scale_factor().unwrap_or(1.0);
        let size = win.inner_size().map_err(|e| format!("inner_size: {e}"))?;
        let w = size.width as f64 / scale;
        let h = size.height as f64 / scale;
        let anchor = StatusBarAnchor {
            center_x: x + w / 2.0,
            bottom_y: y + h,
        };
        save_status_bar_anchor(anchor)?;
        win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
            .map_err(|e| format!("set position failed: {e}"))?;
        Ok(())
    }
}

#[tauri::command]
fn reset_status_bar_position(app: tauri::AppHandle) -> Result<(), String> {
    // Marshal onto the main thread on every platform (see resize_status_bar).
    let app_c = app.clone();
    if let Err(e) = run_on_main_guarded(&app, "status_bar.reset_position", move || {
        if let Err(e) = reset_status_bar_position_on_main(&app_c) {
            tracing::warn!("[status-bar] reset position failed: {e}");
        }
    }) {
        return Err(format!("schedule reset position failed: {e}"));
    }
    Ok(())
}

fn reset_status_bar_position_on_main(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return hud_manager::MacHudManager::new(app).reset_position_on_main();
    }

    #[cfg(not(target_os = "macos"))]
    {
        clear_status_bar_position()?;
        if let Some(win) = app.get_webview_window("status-bar") {
            apply_status_bar_position(app, &win)?;
        }
        Ok(())
    }
}

#[tauri::command]
fn set_status_bar_interactive(app: tauri::AppHandle, interactive: bool) -> Result<(), String> {
    set_status_bar_interactive_state(&app, interactive);
    Ok(())
}

/// Return `{url, secret}` so the frontend can hit the backend directly.
#[tauri::command]
fn get_backend_endpoint(backend: State<'_, BackendState>) -> Result<serde_json::Value, String> {
    let lock = backend.0.lock().map_err(|_| "lock failed")?;
    let ep = lock.as_ref().ok_or("backend not yet started")?;
    Ok(serde_json::json!({ "url": ep.url, "secret": ep.secret }))
}

#[tauri::command]
async fn get_preferences(backend: State<'_, BackendState>) -> Result<api::Preferences, String> {
    let ep = get_endpoint(&backend)?;
    api::get_preferences(&ep).await
}

#[derive(serde::Serialize)]
struct SttRuntimeInfo {
    /// Provider the next dictation will use: live Together Nemotron or the
    /// mandated Apple-Silicon on-device model.
    dictation_provider: &'static str,
    /// Dictation can transcribe right now (model present / key baked).
    dictation_ready: bool,
    /// Effective persisted preference after device-policy normalization.
    dictation_stt_pref: String,
    /// Retained for diagnostics; matches the enforced route on locked devices.
    dictation_auto_provider: &'static str,
    // On-device Oriserve status — powers meetings everywhere and local
    // dictation only on Apple Silicon when the device policy selects it.
    whisper_installed: bool,
    whisper_ready: bool,
    whisper_vad_installed: bool,
    /// Selected local ASR implementation: "oriserve", "nemotron-q4", or
    /// "nemotron-q8". Legacy "nemotron" remains Q8-compatible.
    local_stt_model: String,
    /// Whether the selected optional Nemotron model is fully present on disk.
    nemotron_installed: bool,
}

#[tauri::command]
async fn get_stt_runtime(backend: State<'_, BackendState>) -> Result<SttRuntimeInfo, String> {
    let whisper_installed = dictation_stt::model_installed();
    let whisper_ready = whisper_installed && dictation_stt::runtime_ready();
    let whisper_vad_installed = dictation_stt::vad_installed();
    let desktop_prefs = stt_policy::normalize_prefs(said_core::prefs::load());
    let info = SttRuntimeInfo {
        dictation_provider: dictation_stt::provider_name(),
        dictation_ready: dictation_stt::dictation_ready(),
        dictation_stt_pref: desktop_prefs.dictation_stt,
        dictation_auto_provider: dictation_stt::auto_provider_name(),
        whisper_installed,
        whisper_ready,
        whisper_vad_installed,
        local_stt_model: desktop_prefs.local_stt_model,
        nemotron_installed: nemotron::selected_installed(),
    };

    if let Ok(ep) = get_endpoint(&backend) {
        let _ = api::get_preferences(&ep).await;
    }
    Ok(info)
}

/// Hardware-derived dictation setup policy. Used by onboarding, the forced
/// post-update gate, and Settings; runtime routing consumes the same policy.
#[tauri::command]
fn get_stt_setup_policy() -> stt_policy::SttSetupPolicy {
    stt_policy::current().clone()
}

#[tauri::command]
async fn patch_preferences(
    backend: State<'_, BackendState>,
    tray_cache: State<'_, TrayCache>,
    hot_cache: State<'_, HotPathCache>,
    app: tauri::AppHandle,
    update: api::PrefsUpdate,
) -> Result<api::Preferences, String> {
    tracing::info!(
        "[patch_prefs] Tauri received: llm_provider={:?} selected_model={:?} tone={:?}",
        update.llm_provider,
        update.selected_model,
        update.tone_preset
    );
    let ep = get_endpoint(&backend)?;
    let result = api::patch_preferences(&ep, update).await;
    match &result {
        Ok(p) => {
            tracing::info!(
                "[patch_prefs] backend returned: llm_provider={:?}",
                p.llm_provider
            );
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            hotkey::set_record_hotkey(parse_record_hotkey(&p.record_hotkey));
            // Keep tray cache in sync so sync_tray never needs async
            if let Ok(mut cache) = tray_cache.0.lock() {
                cache.custom_prompt = p.custom_prompt.clone();
                cache.output_language = p.output_language.clone();
                cache.record_hotkey = p.record_hotkey.clone();
            }
            // Re-render tray menu to show updated checkmark
            let shared = app.state::<SharedApp>();
            if let Ok(d) = shared.0.lock() {
                let snap = d.snapshot();
                drop(d);
                sync_tray(&app, &snap);
            }
            // Keep hot-path cache in sync — no HTTP needed next recording.
            let mut hot = hot_cache.0.write().await;
            hot.language = p.language.clone();
            // Let meeting AI use the Groq key saved in Settings → API keys.
            meeting_engine::set_runtime_groq_api_key(p.groq_api_key.clone());
        }
        Err(e) => tracing::warn!("[patch_prefs] backend error: {e}"),
    }
    result
}

#[tauri::command]
async fn get_history(
    backend: State<'_, BackendState>,
    limit: Option<i64>,
    // Pagination cursor: return only recordings older than this timestamp (ms).
    // Lets History load one page at a time instead of the whole table at once.
    before: Option<i64>,
) -> Result<Vec<api::Recording>, String> {
    let ep = get_endpoint(&backend)?;
    api::get_history(&ep, limit.unwrap_or(50), before).await
}

/// Resolve a stored `target_app` key (bundle-id on macOS / exe path on Windows)
/// to a `data:image/png;base64,…` icon URL for the History page. Results are
/// cached in-process, so calling this per row for repeated apps is cheap.
/// Returns `None` when the app can't be found or has no icon (row falls back to
/// the generic placeholder).
#[tauri::command]
async fn get_app_icon(app_key: String) -> Option<String> {
    if app_key.trim().is_empty() {
        return None;
    }
    app_identity::icon_data_url(&app_key)
}

/// Full identity (name + category + icon) for a stored `target_app` key. Used by
/// the Insights "apps you dictate in" section. Cached in-process.
#[tauri::command]
async fn get_app_identity(app_key: String) -> Option<app_identity::AppIdentity> {
    if app_key.trim().is_empty() {
        return None;
    }
    Some(app_identity::describe(&app_key))
}

/// Per-app dictation usage (grouped by target_app) for the Insights section.
#[tauri::command]
async fn get_app_usage(backend: State<'_, BackendState>) -> Result<Vec<api::AppUsage>, String> {
    let ep = get_endpoint(&backend)?;
    api::get_app_usage(&ep).await
}

/// Fire the macOS Automation consent prompt for a browser bundle-id (used by the
/// opt-in "Enable browser context" affordances so the prompt shows up front
/// rather than mid-dictation). Returns true if the event went through (already
/// granted). No-op / false for non-browsers and non-macOS.
#[tauri::command]
async fn trigger_browser_automation(app_key: String) -> bool {
    // Blocks on the native consent dialog — keep it off the async executor.
    tauri::async_runtime::spawn_blocking(move || {
        browser_context::trigger_automation_prompt(&app_key)
    })
    .await
    .unwrap_or(false)
}

/// Prompt macOS Automation consent for all currently-running known browsers, so
/// the user grants it upfront when they enable the feature. Returns the browser
/// names prompted (empty on non-macOS / no browsers running).
#[tauri::command]
async fn request_browser_automation() -> Vec<String> {
    // Prompts each running browser in turn — blocking on user consent.
    tauri::async_runtime::spawn_blocking(browser_context::request_automation_upfront)
        .await
        .unwrap_or_default()
}

/// Live Automation consent state for every known browser currently running, so
/// Settings can show real per-browser status + a working "Grant" button. Empty on
/// non-macOS or when no known browser is running.
#[tauri::command]
async fn browser_automation_status() -> Vec<browser_context::BrowserAutomation> {
    tauri::async_runtime::spawn_blocking(browser_context::running_browser_automation)
        .await
        .unwrap_or_default()
}

/// Per-site dictation usage (grouped by host) for the Insights "Sites" section.
#[tauri::command]
async fn get_site_usage(backend: State<'_, BackendState>) -> Result<Vec<api::SiteUsage>, String> {
    let ep = get_endpoint(&backend)?;
    api::get_site_usage(&ep).await
}

/// Favicon for a site host as a `data:` URL (direct fetch, cached). `None` →
/// the frontend draws a letter-tile fallback.
#[tauri::command]
async fn get_favicon(host: String) -> Option<String> {
    favicon::favicon_data_url(&host).await
}

#[tauri::command]
async fn submit_edit_feedback(
    backend: State<'_, BackendState>,
    recording_id: String,
    user_kept: String,
    target_app: Option<String>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::submit_feedback(&ep, &recording_id, &user_kept, target_app.as_deref()).await
}

#[tauri::command]
fn set_mode(
    _key: String,
    state: State<'_, SharedApp>,
    app: tauri::AppHandle,
) -> Result<AppSnapshot, String> {
    // Model switching removed — always uses gpt-5.4-mini.
    let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
    sync_tray(&app, &snap);
    Ok(snapshot_for_ui(&app, &snap))
}

#[tauri::command]
fn request_accessibility(
    state: State<'_, SharedApp>,
    app: tauri::AppHandle,
) -> Result<AppSnapshot, String> {
    paster::request_permission();
    let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
    Ok(snapshot_for_ui(&app, &snap))
}

#[tauri::command]
fn request_input_monitoring(
    state: State<'_, SharedApp>,
    app: tauri::AppHandle,
) -> Result<AppSnapshot, String> {
    paster::request_input_monitoring();
    let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
    Ok(snapshot_for_ui(&app, &snap))
}

#[tauri::command]
fn request_microphone(
    state: State<'_, SharedApp>,
    app: tauri::AppHandle,
) -> Result<AppSnapshot, String> {
    permissions::request_microphone();
    let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
    Ok(snapshot_for_ui(&app, &snap))
}

/// Open the OS privacy pane for the microphone so the user can flip access on by
/// hand — the recovery path when a grant prompt was dismissed/denied and the
/// onboarding step is otherwise stuck "waiting for grants". macOS opens the
/// System Settings anchor; Windows opens the microphone privacy page.
#[tauri::command]
fn open_microphone_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:privacy-microphone"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}

/// Open the macOS Accessibility privacy pane (recovery path for a denied grant).
/// No-op off macOS — Windows/Linux don't gate HID typing behind this permission.
#[tauri::command]
fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
            .spawn();
    }
}

/// Open the macOS Input Monitoring privacy pane (recovery path for a denied
/// grant). No-op off macOS — Windows/Linux don't require this permission.
#[tauri::command]
fn open_input_monitoring_settings() {
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            .spawn();
    }
}

/// Whether Screen Recording is already granted (no prompt). Used to gate meeting
/// start, since system-audio capture goes through ScreenCaptureKit.
#[tauri::command]
fn screen_recording_granted() -> bool {
    permissions::screen_recording_granted()
}

/// Check Screen Recording; if not granted, raise the macOS prompt + open the pane.
/// Returns the resulting grant state (often `false` until the app is relaunched).
#[tauri::command]
fn request_screen_recording() -> bool {
    permissions::request_screen_recording()
}

/// Run the 5-method AX field reading diagnostic on whatever is focused right now.
/// The Tauri app already has Accessibility permission, so unlike a fresh standalone
/// binary, this can always reach the focused application.
///
/// `delay_secs` is how long to wait before sampling — gives the user time to
/// click into the target app before the diagnostic runs.
#[tauri::command]
async fn diagnose_ax(delay_secs: u64) -> Result<paster::AxDiagnostics, String> {
    let delay = delay_secs.clamp(0, 30);
    if delay > 0 {
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
    }
    // Run the (synchronous, FFI-heavy) diagnostic on a blocking thread.
    let report = tokio::task::spawn_blocking(paster::diagnose_focused_field)
        .await
        .map_err(|e| format!("diagnostic task failed: {e}"))?;
    Ok(report)
}

/// UI button: start or stop recording depending on current state.
/// - idle      → start recording, return snapshot with state="recording"
/// - recording → stop recording, kick off async SSE pipeline, return state="processing"
/// - processing → no-op (return current snapshot)
#[tauri::command]
fn toggle_recording(
    state: State<'_, SharedApp>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<AppSnapshot, String> {
    let current_state = state.0.lock().map_err(|_| "lock failed")?.state;

    match current_state {
        desktop::AppState::Idle => {
            do_start_recording(&state.0, &app);
            let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
            Ok(snapshot_for_ui(&app, &snap))
        }
        desktop::AppState::Recording => {
            do_finish_recording(Arc::clone(&state.0), app.clone(), Arc::clone(&backend.0));
            let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
            Ok(snapshot_for_ui(&app, &snap))
        }
        desktop::AppState::Processing => {
            // Already in flight — return current snapshot, don't do anything
            let snap = state.0.lock().map_err(|_| "lock failed")?.snapshot();
            Ok(snapshot_for_ui(&app, &snap))
        }
    }
}

/// Panel "Speak follow-up" (press-and-hold): start a Divo-routed recording that,
/// on release, is sent as a follow-up on the active thread rather than a new task.
#[tauri::command]
fn divo_followup_begin(state: State<'_, SharedApp>, app: tauri::AppHandle) -> Result<(), String> {
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    if current == desktop::AppState::Idle {
        DIVO_START_PENDING.store(true, Ordering::SeqCst);
        DIVO_FOLLOWUP_PENDING.store(true, Ordering::SeqCst);
        DIVO_NEW_CHAT_PENDING.store(false, Ordering::SeqCst);
        do_start_recording(&state.0, &app);
    }
    Ok(())
}

/// Release of the panel follow-up button — finish recording and send to Divo.
#[tauri::command]
fn divo_followup_end(
    state: State<'_, SharedApp>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    if current == desktop::AppState::Recording {
        do_finish_recording(Arc::clone(&state.0), app.clone(), Arc::clone(&backend.0));
    }
    Ok(())
}

#[tauri::command]
fn developer_problem_begin(
    state: State<'_, SharedApp>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let settings = developer_context::load_settings();
    if !settings.enabled {
        return Err("Developer Problem Command is disabled in Settings → Developer".to_string());
    }
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    if current == desktop::AppState::Idle {
        PROBLEM_START_PENDING.store(true, Ordering::SeqCst);
        do_start_recording(&state.0, &app);
    }
    Ok(())
}

#[tauri::command]
fn developer_problem_end(
    state: State<'_, SharedApp>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    if current == desktop::AppState::Recording {
        do_finish_recording(Arc::clone(&state.0), app.clone(), Arc::clone(&backend.0));
    }
    Ok(())
}

#[tauri::command]
fn developer_problem_dismiss(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(pending) = app.try_state::<PendingProblemState>() {
        if let Ok(mut slot) = pending.0.lock() {
            *slot = None;
        }
    }
    let _ = app.emit(
        "voice-output",
        serde_json::json!({"status": "manual_paste", "message": "Problem command cancelled"}),
    );
    Ok(())
}

#[tauri::command]
fn developer_problem_choose_project(
    project_id: String,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let pending = app
        .try_state::<PendingProblemState>()
        .ok_or_else(|| "no pending problem command".to_string())?;
    let command = {
        let mut slot = pending
            .0
            .lock()
            .map_err(|_| "pending problem lock failed")?;
        let Some(command) = slot.take() else {
            return Err("no pending problem command".to_string());
        };
        if now_ms_desktop().saturating_sub(command.created_at_ms as u64) > 5 * 60 * 1000 {
            return Err("pending problem command expired — please record again".to_string());
        }
        command
    };
    let Some(project) = command
        .candidates
        .iter()
        .find(|candidate| candidate.id == project_id)
        .cloned()
    else {
        return Err("project is not a pending candidate".to_string());
    };

    let back_arc = Arc::clone(&backend.0);
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = solve_problem_with_project(
            &back_arc,
            &app_clone,
            command.transcript,
            command.screen_context,
            command.client_run_id,
            command.edit_target_pid,
            Some(project),
        )
        .await;
        if let Err(err) = result {
            emit_voice_error_quiet(&app_clone, &err);
        }
    });
    Ok(())
}

// ── Recording flow ────────────────────────────────────────────────────────────

/// Guards against overlapping start-start or start-cancel-start races.
/// The start→finish pair is allowed through (finish clears the flag),
/// but a second START while the first hasn't finished is rejected.
static RECORDING_STARTING: AtomicBool = AtomicBool::new(false);
static HOTKEY_START_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static FINISH_AFTER_START: AtomicBool = AtomicBool::new(false);
static HOTKEY_FINISH_RETRY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
/// Set by the Ctrl press (or the panel follow-up command) right before
/// `do_start_recording`, consumed there to route the turn to Divo.
static DIVO_START_PENDING: AtomicBool = AtomicBool::new(false);
/// Set by the Developer Command trigger right before `do_start_recording`,
/// consumed there to route the turn to the isolated Problem flow.
static PROBLEM_START_PENDING: AtomicBool = AtomicBool::new(false);
/// Distinguishes a spoken follow-up (continue the current thread) from a fresh
/// Ctrl press (new task). Consumed in `do_finish_recording`'s Divo branch.
static DIVO_FOLLOWUP_PENDING: AtomicBool = AtomicBool::new(false);
/// Set on release when the capture was a Ctrl+N hold — the staged turn defaults to
/// a brand-new chat. Captured from the hotkey crate at release, read at staging.
static DIVO_NEW_CHAT_PENDING: AtomicBool = AtomicBool::new(false);

/// Minimum time between consecutive finish→start cycles (ms).
/// Prevents rapid Caps Lock taps from flooding the recording pipeline.
static LAST_FINISH_MS: AtomicU64 = AtomicU64::new(0);
const MIN_CYCLE_GAP_MS: u64 = 300;
const QUEUED_FINISH_TIMEOUT_MS: u64 = 2_500;
const QUEUED_FINISH_POLL_MS: u64 = 25;
const RELEASE_MIC_CLEANUP_DELAY_MS: u64 = 850;
const RELEASE_MIC_CLEANUP_RECHECK_MS: u64 = 1_800;
const DICTATION_RELEASE_TAIL_MS: u64 = 400;
const DICTATION_RELEASE_TAIL_MAX_MS: u64 = 1_000;

fn now_ms_desktop() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn dictation_release_tail_ms() -> u64 {
    std::env::var("AIRNOTE_DICTATION_RELEASE_TAIL_MS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DICTATION_RELEASE_TAIL_MS)
        .min(DICTATION_RELEASE_TAIL_MAX_MS)
}

fn maybe_pad_dictation_release(app: &tauri::AppHandle) {
    let route = app
        .try_state::<RecordingRouteState>()
        .and_then(|route| route.0.lock().ok().and_then(|guard| *guard));
    if route != Some(RecordingRoute::Normal) {
        return;
    }

    let tail_ms = dictation_release_tail_ms();
    if tail_ms == 0 {
        return;
    }

    tracing::info!("[record] release tail pad before stop tail_ms={tail_ms}");
    diag::breadcrumb(format!("release_tail:{tail_ms}ms"));
    std::thread::sleep(Duration::from_millis(tail_ms));
}

// Poll cadence and total budget for reading the shared app state from a
// hotkey-spawned thread. A press/release is handled on its own detached thread
// (the CGEventTap callback has already returned), so waiting here is safe — and
// dropping a genuine keypress is far worse than blocking briefly. The budget
// must comfortably exceed how long `start_recording` holds the shared lock while
// CoreAudio opens the input stream: `recorder.start()` blocks on the stream
// build + `play()` (a syscall that routinely takes a few hundred ms, more when
// Bluetooth routing is reconfigured) all while the mutex is held. At 200ms the
// release/finish state read was timing out mid-open, returning `None` and
// forcing the lossy queued-finish fallback even though the user held the key
// properly. The cap stays bounded (not an infinite block) so a genuinely wedged
// lock still escapes to the `None` recovery path.
const HOTKEY_STATE_LOCK_POLL_MS: u64 = 20;
const HOTKEY_STATE_LOCK_BUDGET_MS: u64 = 2_000;

fn hotkey_current_state(shared: &Arc<Mutex<DesktopApp>>, label: &str) -> Option<desktop::AppState> {
    let attempts = HOTKEY_STATE_LOCK_BUDGET_MS / HOTKEY_STATE_LOCK_POLL_MS;
    for attempt in 0..attempts {
        if let Ok(d) = shared.try_lock() {
            return Some(d.state);
        }
        if attempt == 0 {
            tracing::debug!("[hotkey] {label} waiting for shared app lock");
        }
        std::thread::sleep(std::time::Duration::from_millis(HOTKEY_STATE_LOCK_POLL_MS));
    }
    tracing::warn!(
        "[hotkey] {label} skipped — shared app lock busy for {HOTKEY_STATE_LOCK_BUDGET_MS}ms"
    );
    diag::breadcrumb(format!("lock_busy:{label}"));
    None
}

/// RAII guard over the SharedApp mutex that publishes the holder label to
/// [`diag`] on acquire and clears it on drop. Lets stuck-state diagnostics name
/// the thread that is actually holding the lock (e.g. a slow CoreAudio open
/// inside `start_recording`) instead of only reporting `lock_busy`.
struct TrackedGuard<'a> {
    inner: std::sync::MutexGuard<'a, DesktopApp>,
}

impl Drop for TrackedGuard<'_> {
    fn drop(&mut self) {
        diag::note_lock_released();
    }
}

impl std::ops::Deref for TrackedGuard<'_> {
    type Target = DesktopApp;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for TrackedGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

/// Acquire the SharedApp mutex while recording the holder for diagnostics.
/// `label` is a fixed identifier such as `"start_recording"`.
fn lock_shared<'a>(
    shared: &'a Arc<Mutex<DesktopApp>>,
    label: &'static str,
) -> Result<TrackedGuard<'a>, std::sync::PoisonError<std::sync::MutexGuard<'a, DesktopApp>>> {
    // Recover from a poisoned lock instead of failing. A poisoned mutex means a
    // previous holder panicked; if we propagated the error, every future
    // recording would silently refuse to start (the lock would be permanently
    // wedged). The state watchdog resets any half-updated state back to idle, so
    // taking the (possibly inconsistent) data here is safe and keeps the app usable.
    let inner = shared.lock().unwrap_or_else(|poison| {
        tracing::warn!("[lock] recovering poisoned SharedApp lock for {label}");
        diag::breadcrumb("lock:poison_recovered");
        poison.into_inner()
    });
    diag::note_lock_acquired(label);
    Ok(TrackedGuard { inner })
}

/// Abandon the in-flight `Processing` run and reset to Idle so a fresh recording
/// can start immediately. This is an INTENTIONAL user action (they pressed record
/// again to redo a mis-released take), so — unlike [`heal_stuck_state`] — it emits
/// no error telemetry and deliberately does NOT recover the orphan audio: the take
/// is being discarded, and the superseded pipeline's paste is already suppressed
/// via the recording-generation bump in `do_start_recording`. No-op unless the
/// state is actually `Processing`.
fn cancel_processing_run(app: &tauri::AppHandle, reason: &'static str) {
    let snap = {
        let shared = app.state::<SharedApp>();
        let mut d = match shared.0.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        d.recover_stuck_to_idle()
    };
    let Some(snap) = snap else {
        return;
    };
    FINISH_AFTER_START.store(false, Ordering::SeqCst);
    HOTKEY_START_IN_FLIGHT.store(false, Ordering::SeqCst);
    HOTKEY_FINISH_RETRY_IN_FLIGHT.store(false, Ordering::SeqCst);
    RECORDING_STARTING.store(false, Ordering::SeqCst);
    restore_speaker_suppression(app, "record pressed during processing");
    reset_long_dictation_lock(app);
    tracing::info!("[record] processing run abandoned, reset to idle (reason={reason})");
    diag::breadcrumb(format!("cancel_processing:{reason}"));
    sync_tray(app, &snap);
    emit_app_state(app, &snap);
    sync_status_bar(app, "idle");
}

/// Heal a wedged app after an operation was abandoned mid-way (a caught panic, a
/// killed pipeline task, a lost event). Resets a stuck `Processing` state to Idle,
/// clears the in-flight recording guards, restores audio/HUD/tray, and recovers
/// the captured audio so the user's words aren't lost. Safe no-op when nothing is
/// stuck — only `Processing` is reset, never an active `Recording`.
fn heal_stuck_state(app: &tauri::AppHandle, reason: &'static str) {
    let snap = {
        let shared = app.state::<SharedApp>();
        let mut d = match shared.0.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        d.recover_stuck_to_idle()
    };
    let Some(snap) = snap else {
        return;
    };

    // Clear in-flight guards so the next recording can start cleanly.
    FINISH_AFTER_START.store(false, Ordering::SeqCst);
    HOTKEY_START_IN_FLIGHT.store(false, Ordering::SeqCst);
    HOTKEY_FINISH_RETRY_IN_FLIGHT.store(false, Ordering::SeqCst);
    RECORDING_STARTING.store(false, Ordering::SeqCst);
    restore_speaker_suppression(app, "heal stuck state");
    reset_long_dictation_lock(app);

    tracing::warn!("[heal] stuck 'processing' state reset to idle (reason={reason})");
    diag::breadcrumb(format!("heal:reset_idle:{reason}"));
    said_core::reporter::report_event(
        "state.healed",
        said_core::reporter::Severity::Error,
        serde_json::json!({ "reason": reason, "trail": diag::breadcrumbs(20) }),
    );

    sync_tray(app, &snap);
    emit_app_state(app, &snap);
    sync_status_bar(app, "idle");

    // The dictation's audio is still on disk (the pipeline never reached cleanup).
    // Recover the user's words in-session. `take_orphan()` returns None if the
    // pipeline actually finished and cleared it, so this never double-processes.
    let app_h = app.clone();
    let back = Arc::clone(&app.state::<BackendState>().0);
    tauri::async_runtime::spawn(async move {
        let Some(wav) = recovery::take_orphan() else {
            return;
        };
        let ep = match back.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        };
        let Some(ep) = ep else {
            return;
        };
        match recover_transcribe(&ep, wav).await {
            Ok(done) => {
                let text = if !done.polished.trim().is_empty() {
                    done.polished
                } else {
                    done.transcript
                };
                if text.trim().is_empty() {
                    return;
                }
                tracing::warn!(
                    "[heal] recovered {} chars from the interrupted dictation",
                    text.len()
                );
                diag::breadcrumb("heal:words_recovered");
                if let Ok(mut g) = app_h.state::<LatestResult>().0.lock() {
                    *g = Some(text.clone());
                }
                show_main_window(&app_h);
                let _ = app_h.emit("dictation-recovered", serde_json::json!({ "text": text }));
            }
            Err(e) => tracing::warn!("[heal] re-transcribe failed: {e}"),
        }
    });
}

fn request_queued_finish(
    shared: Arc<Mutex<DesktopApp>>,
    app: tauri::AppHandle,
    back: Arc<Mutex<Option<BackendEndpoint>>>,
    reason: &'static str,
) {
    FINISH_AFTER_START.store(true, Ordering::SeqCst);
    diag::breadcrumb(format!("queued_finish:request:{reason}"));
    if HOTKEY_FINISH_RETRY_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        tracing::debug!("[hotkey] queued finish already has a retry worker — reason={reason}");
        return;
    }

    std::thread::spawn(move || {
        struct RetryGuard;
        impl Drop for RetryGuard {
            fn drop(&mut self) {
                HOTKEY_FINISH_RETRY_IN_FLIGHT.store(false, Ordering::SeqCst);
            }
        }
        let _guard = RetryGuard;
        let started = std::time::Instant::now();

        loop {
            if !FINISH_AFTER_START.load(Ordering::SeqCst) {
                return;
            }

            let state = shared.try_lock().ok().map(|d| d.state);
            let last_state = match state {
                Some(desktop::AppState::Recording) => {
                    if FINISH_AFTER_START.swap(false, Ordering::SeqCst) {
                        let waited_ms = started.elapsed().as_millis() as u64;
                        tracing::info!(
                            "[hotkey] queued finish recovered after {waited_ms}ms — reason={reason}"
                        );
                        if waited_ms >= 250 {
                            said_core::reporter::report_event(
                                "hotkey.queued_finish_recovered",
                                said_core::reporter::Severity::Warning,
                                serde_json::json!({
                                    "wait_ms": waited_ms,
                                    "reason": reason,
                                    "lock": diag::lock_status(),
                                    "trail": diag::breadcrumbs(20),
                                }),
                            );
                        }
                        do_finish_recording(shared, app, back);
                    }
                    return;
                }
                Some(desktop::AppState::Processing) => {
                    FINISH_AFTER_START.store(false, Ordering::SeqCst);
                    tracing::debug!("[hotkey] queued finish cleared — already processing");
                    return;
                }
                Some(desktop::AppState::Idle) => {
                    if !HOTKEY_START_IN_FLIGHT.load(Ordering::SeqCst)
                        && !RECORDING_STARTING.load(Ordering::SeqCst)
                    {
                        FINISH_AFTER_START.store(false, Ordering::SeqCst);
                        tracing::debug!(
                            "[hotkey] queued finish cleared — start never reached recording"
                        );
                        return;
                    }
                    "idle"
                }
                None => "lock_busy",
            };

            let waited_ms = started.elapsed().as_millis() as u64;
            if waited_ms >= QUEUED_FINISH_TIMEOUT_MS {
                if FINISH_AFTER_START.swap(false, Ordering::SeqCst) {
                    tracing::error!(
                        "[hotkey] queued finish timed out after {waited_ms}ms — reason={reason} state={last_state}"
                    );
                    let session = app
                        .try_state::<RecordingSessionState>()
                        .and_then(|s| s.current());
                    diag::breadcrumb("queued_finish:timeout");
                    said_core::reporter::report_event(
                        "hotkey.queued_finish_timeout",
                        said_core::reporter::Severity::Error,
                        serde_json::json!({
                            "wait_ms": waited_ms,
                            "reason": reason,
                            "state": last_state,
                            "lock": diag::lock_status(),
                            "session_id": session.as_ref().map(|(_, id)| id.clone()),
                            "generation": session.as_ref().map(|(g, _)| *g),
                            "trail": diag::breadcrumbs(20),
                        }),
                    );
                }
                return;
            }

            std::thread::sleep(std::time::Duration::from_millis(QUEUED_FINISH_POLL_MS));
        }
    });
}

fn schedule_release_mic_cleanup(
    shared: Arc<Mutex<DesktopApp>>,
    app: tauri::AppHandle,
    back: Arc<Mutex<Option<BackendEndpoint>>>,
    reason: &'static str,
) {
    std::thread::spawn(move || {
        for (attempt, delay_ms) in [
            (1usize, RELEASE_MIC_CLEANUP_DELAY_MS),
            (2usize, RELEASE_MIC_CLEANUP_RECHECK_MS),
        ] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));

            if app
                .try_state::<LongDictationState>()
                .map(|s| s.locked.load(Ordering::SeqCst))
                .unwrap_or(false)
            {
                tracing::debug!(
                    "[mic-cleanup] skipped after release — long dictation is intentionally locked"
                );
                return;
            }

            let state_and_mic = shared
                .try_lock()
                .ok()
                .map(|d| (d.state, d.mic_stream_held()));
            let Some((state, mic_held)) = state_and_mic else {
                tracing::debug!(
                    "[mic-cleanup] shared app lock busy after release attempt={attempt} reason={reason}"
                );
                continue;
            };

            match state {
                desktop::AppState::Recording => {
                    let route = app
                        .try_state::<RecordingRouteState>()
                        .and_then(|route| route.0.lock().ok().and_then(|g| *g));
                    if route == Some(RecordingRoute::Meeting) {
                        tracing::debug!(
                            "[mic-cleanup] skipped after release — meeting capture owns recording"
                        );
                        return;
                    }

                    tracing::warn!(
                        "[mic-cleanup] release left recorder in recording state — finishing now attempt={attempt} reason={reason}"
                    );
                    diag::breadcrumb(format!("mic_cleanup:finish:{reason}:{attempt}"));
                    said_core::reporter::report_event(
                        "mic_cleanup.finish_after_release",
                        said_core::reporter::Severity::Warning,
                        serde_json::json!({
                            "attempt": attempt,
                            "reason": reason,
                            "trail": diag::breadcrumbs(20),
                        }),
                    );
                    do_finish_recording(shared, app, back);
                    return;
                }
                desktop::AppState::Processing | desktop::AppState::Idle if mic_held => {
                    let stop_rx = {
                        let mut d = match shared.lock() {
                            Ok(g) => g,
                            Err(p) => p.into_inner(),
                        };
                        d.release_mic_stream()
                    };
                    if let Some(stop_rx) = stop_rx {
                        tracing::warn!(
                            "[mic-cleanup] forced stale mic stream release state={} attempt={} reason={}",
                            state.as_str(),
                            attempt,
                            reason
                        );
                        diag::breadcrumb(format!("mic_cleanup:force_release:{reason}:{attempt}"));
                        std::thread::spawn(move || {
                            let _ = stop_rx.recv_timeout(std::time::Duration::from_secs(3));
                        });
                    }
                    return;
                }
                _ => {
                    if attempt == 2 {
                        tracing::debug!(
                            "[mic-cleanup] release cleanup clean state={} mic_held={} reason={reason}",
                            state.as_str(),
                            mic_held
                        );
                    }
                }
            }
        }
    });
}

/// Start recording. Called when user presses Caps Lock (or taps the button).
fn do_start_recording(shared: &Arc<Mutex<DesktopApp>>, app: &tauri::AppHandle) {
    do_start_recording_inner(shared, app, true);
}

fn do_start_recording_without_cycle_gap(shared: &Arc<Mutex<DesktopApp>>, app: &tauri::AppHandle) {
    do_start_recording_inner(shared, app, false);
}

fn do_start_recording_inner(
    shared: &Arc<Mutex<DesktopApp>>,
    app: &tauri::AppHandle,
    enforce_cycle_gap: bool,
) {
    diag::breadcrumb("record:start:enter");
    // Reject if another start is already in progress
    if RECORDING_STARTING.swap(true, Ordering::SeqCst) {
        diag::breadcrumb("record:start:skip_in_flight");
        tracing::info!("[record] start skipped — another start already in progress");
        DIVO_START_PENDING.store(false, Ordering::SeqCst);
        PROBLEM_START_PENDING.store(false, Ordering::SeqCst);
        return;
    }

    // Reject rapid re-entry after a recent finish
    let now = now_ms_desktop();
    let last_finish = LAST_FINISH_MS.load(Ordering::SeqCst);
    if enforce_cycle_gap && last_finish > 0 && now.saturating_sub(last_finish) < MIN_CYCLE_GAP_MS {
        diag::breadcrumb("record:start:skip_cycle_gap");
        tracing::info!(
            "[record] start skipped — too soon after last finish ({}ms < {}ms)",
            now.saturating_sub(last_finish),
            MIN_CYCLE_GAP_MS,
        );
        FINISH_AFTER_START.store(false, Ordering::SeqCst);
        RECORDING_STARTING.store(false, Ordering::SeqCst);
        DIVO_START_PENDING.store(false, Ordering::SeqCst);
        PROBLEM_START_PENDING.store(false, Ordering::SeqCst);
        return;
    }

    // Clear the flag when this function returns (success or failure)
    struct StartGuard;
    impl Drop for StartGuard {
        fn drop(&mut self) {
            RECORDING_STARTING.store(false, Ordering::SeqCst);
        }
    }
    let _guard = StartGuard;

    finalize_edit_watcher(app, "new recording");

    let meeting_capture = app
        .try_state::<MeetingModeState>()
        .map(|s| s.capture_enabled())
        .unwrap_or(false);

    #[cfg(target_os = "macos")]
    if !meeting_capture {
        if let Some(guard) = app.try_state::<speaker_suppression::SpeakerSuppressionGuard>() {
            guard.begin("normal dictation start");
        }
    }

    let arm_started = std::time::Instant::now();
    diag::breadcrumb("start:lock_acquire");
    let (started, level_recv) = match lock_shared(shared, "start_recording") {
        Ok(mut d) => {
            let result = d.start_recording();
            let lr = if result.is_ok() {
                d.take_level_receiver()
            } else {
                None
            };
            (result, lr)
        }
        Err(_) => {
            restore_speaker_suppression(app, "start lock failed");
            return;
        }
    };

    let snap = match started {
        Ok(snap) => {
            tracing::info!(
                "[record] audio armed in {}ms",
                arm_started.elapsed().as_millis()
            );
            snap
        }
        Err(e) => {
            diag::breadcrumb("start:failed");
            restore_speaker_suppression(app, "start failed");
            FINISH_AFTER_START.store(false, Ordering::SeqCst);
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": e,
                    "audio_id": null,
                }),
            );
            return;
        }
    };

    // Lock and pre-unlock the frontmost app's accessibility tree after audio is
    // armed. Chrome / Electron still get the whole dictation window to warm up,
    // while the user's first syllable is already being captured.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        if !meeting_capture {
            let context_started = std::time::Instant::now();
            let pid = paster::lock_frontmost_app_now();
            tracing::debug!("[record] locked frontmost app for edit-watch pid={pid:?}");
            if let Ok(mut target) = app.state::<EditTargetState>().0.lock() {
                *target = pid;
            }
            // Capture the text currently in the focused field. This gives the
            // polish LLM surrounding context for smarter STT corrections —
            // e.g. if the field already mentions "MACOBS", the LLM knows
            // "main corps" in the transcript is likely "MACOBS".
            let screen_text = match pid {
                Some(p) => paster::read_focused_value_fast_for_pid(p),
                None => paster::read_focused_value_fast(),
            };
            if let Ok(mut ctx) = app.state::<ScreenContextState>().0.lock() {
                *ctx = screen_text.filter(|s| !s.trim().is_empty());
                if let Some(text) = ctx.as_ref() {
                    tracing::info!(
                        "[record] screen context: {} chars in {}ms",
                        text.len(),
                        context_started.elapsed().as_millis()
                    );
                }
            }
        }
    }

    {
        let (session_gen, run_id) = app.state::<RecordingSessionState>().begin();
        let route = if DIVO_START_PENDING.swap(false, Ordering::SeqCst) {
            RecordingRoute::Divo
        } else if PROBLEM_START_PENDING.swap(false, Ordering::SeqCst) {
            RecordingRoute::Problem
        } else {
            app.try_state::<MeetingModeState>()
                .map(|s| {
                    if s.capture_enabled() {
                        RecordingRoute::Meeting
                    } else {
                        RecordingRoute::Normal
                    }
                })
                .unwrap_or(RecordingRoute::Normal)
        };
        if let Ok(mut route_state) = app.state::<RecordingRouteState>().0.lock() {
            *route_state = Some(route);
        }
        // Every hold-to-talk route uses the live cloud session when Nemotron
        // is selected. Meetings deliberately remain on their independent,
        // local-only continuous-capture engine.
        if route != RecordingRoute::Meeting {
            start_live_nemotron_session(app, session_gen, run_id.clone());
        }
        let mode = match route {
            RecordingRoute::Divo => "divo",
            RecordingRoute::Problem => "developer_problem",
            RecordingRoute::Meeting => "meeting",
            RecordingRoute::Normal if said_core::prefs::load().message_polish_mode => {
                "message_polish"
            }
            RecordingRoute::Normal => "normal_voice",
        };
        if let Some(ep) = app
            .try_state::<BackendState>()
            .and_then(|b| b.0.lock().ok().and_then(|g| g.clone()))
        {
            telemetry::on_run_start(&ep, &run_id, mode, None);
        }
        tracing::info!("[record] started — state={}", snap.state);
        diag::breadcrumb("start:recording");
        sync_tray(app, &snap);
        emit_app_state(app, &snap);
        if app
            .try_state::<LongDictationState>()
            .map(|s| s.pending_lock.swap(false, Ordering::SeqCst))
            .unwrap_or(false)
        {
            activate_long_dictation_lock(app);
        }
        emit_meeting_stt_status(app);
    }

    // Drive the floating HUD visualizer from the same microphone samples used
    // by recording. It stays independent from speech recognition so the UI
    // remains responsive while local ASR runs after release.
    let level_recv = level_recv;
    if let Some(level_recv) = level_recv {
        let app_levels = app.clone();
        let shared_for_levels = Arc::clone(shared);
        let meeting_pause = app.try_state::<MeetingModeState>().and_then(|meeting| {
            if !meeting.capture_enabled() {
                return None;
            }
            Some((
                Arc::clone(&meeting.active),
                Arc::clone(&meeting.muted),
                Arc::clone(&meeting.generation),
                meeting.generation.load(Ordering::SeqCst),
                Arc::clone(&meeting.echo_gate),
                Arc::clone(shared),
                app.clone(),
                Arc::clone(&app.state::<BackendState>().0),
            ))
        });
        std::thread::spawn(move || {
            let mut smoothed = 0.0f32;
            let mut last_emit = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_millis(40))
                .unwrap_or_else(std::time::Instant::now);
            let started_at = std::time::Instant::now();
            let mut heard_speech = false;
            let mut last_voice_at = started_at;
            let mut finish_for_pause = false;
            loop {
                let level = match level_recv
                    .rx
                    .recv_timeout(std::time::Duration::from_millis(250))
                {
                    Ok(level) => level,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        let still_recording = match shared_for_levels.try_lock() {
                            Ok(d) => d.state == desktop::AppState::Recording,
                            Err(std::sync::TryLockError::WouldBlock) => true,
                            Err(std::sync::TryLockError::Poisoned(poison)) => {
                                poison.into_inner().state == desktop::AppState::Recording
                            }
                        };
                        if !still_recording {
                            tracing::warn!(
                                "[record] level stream still open after recording ended — stopping visualizer"
                            );
                            break;
                        }
                        continue;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };

                let still_recording = match shared_for_levels.try_lock() {
                    Ok(d) => d.state == desktop::AppState::Recording,
                    Err(std::sync::TryLockError::WouldBlock) => true,
                    Err(std::sync::TryLockError::Poisoned(poison)) => {
                        poison.into_inner().state == desktop::AppState::Recording
                    }
                };
                if !still_recording {
                    tracing::warn!(
                        "[record] received mic level after recording ended — stopping visualizer"
                    );
                    break;
                }

                smoothed = smoothed.mul_add(0.68, level * 0.32);
                if last_emit.elapsed() >= std::time::Duration::from_millis(33) {
                    let _ = app_levels.emit(
                        "voice-level",
                        serde_json::json!({
                            "level": smoothed.clamp(0.0, 1.0),
                        }),
                    );
                    last_emit = std::time::Instant::now();
                }

                if let Some((
                    active,
                    muted,
                    generation,
                    expected_generation,
                    echo_gate,
                    shared,
                    _,
                    _,
                )) = &meeting_pause
                {
                    if !active.load(Ordering::SeqCst)
                        || muted.load(Ordering::SeqCst)
                        || generation.load(Ordering::SeqCst) != *expected_generation
                    {
                        break;
                    }

                    let now = std::time::Instant::now();
                    let speech_active = if echo_gate.is_filter_available() {
                        echo_gate.local_speech_active()
                    } else {
                        level >= MEETING_SPEECH_LEVEL
                    };
                    let silence_active = if echo_gate.is_filter_available() {
                        !echo_gate.local_speech_active()
                    } else {
                        level <= MEETING_SILENCE_LEVEL
                    };
                    if speech_active {
                        heard_speech = true;
                        last_voice_at = now;
                    } else if heard_speech
                        && silence_active
                        && now.duration_since(last_voice_at)
                            >= std::time::Duration::from_millis(MEETING_PAUSE_MS)
                        && now.duration_since(started_at)
                            >= std::time::Duration::from_millis(MEETING_MIN_CHUNK_MS)
                    {
                        let still_recording = hotkey_current_state(shared, "meeting pause")
                            == Some(desktop::AppState::Recording);
                        if still_recording {
                            tracing::info!("[meeting_mode] pause detected — finishing chunk");
                            finish_for_pause = true;
                        }
                        break;
                    }
                }
            }
            let _ = app_levels.emit("voice-level", serde_json::json!({ "level": 0.0 }));
            if finish_for_pause {
                if let Some((_, _, _, _, _, shared, app, backend)) = meeting_pause {
                    do_finish_recording(shared, app, backend);
                }
            }
        });
    }

    // ── Meeting time ceiling: force-finalize after MEETING_MAX_CHUNK_MS ────────
    // Prevents unbounded RAM accumulation when someone talks non-stop.
    if let Some(meeting) = app.try_state::<MeetingModeState>() {
        if meeting.capture_enabled() {
            let active = Arc::clone(&meeting.active);
            let muted = Arc::clone(&meeting.muted);
            let generation = Arc::clone(&meeting.generation);
            let expected_gen = meeting.generation.load(Ordering::SeqCst);
            let shared_timer = Arc::clone(shared);
            let app_timer = app.clone();
            let backend_timer = Arc::clone(&app.state::<BackendState>().0);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(MEETING_MAX_CHUNK_MS)).await;
                if active.load(Ordering::SeqCst)
                    && !muted.load(Ordering::SeqCst)
                    && generation.load(Ordering::SeqCst) == expected_gen
                {
                    let still_recording =
                        hotkey_current_state(&shared_timer, "meeting time ceiling")
                            == Some(desktop::AppState::Recording);
                    if still_recording {
                        tracing::info!(
                            "[meeting_mode] time ceiling ({}s) — finishing chunk",
                            MEETING_MAX_CHUNK_MS / 1000
                        );
                        do_finish_recording(shared_timer, app_timer, backend_timer);
                    }
                }
            });
        }
    }

    // Keep a local PCM tap for crash recovery. On-device speech recognition
    // still runs once after release; when Together Nemotron is selected the
    // same capture drain also feeds its WebSocket session while the key is held.
    let chunk_recv = shared.lock().ok().and_then(|mut d| d.take_chunk_receiver());
    if let Some(chunk_recv) = chunk_recv {
        // Crash-safe recovery: capture this dictation's audio to disk so a crash
        // before delivery can be recovered on next launch. Meeting capture is a
        // long continuous stream that auto-restarts, so we skip it there.
        let is_meeting_capture = app
            .try_state::<MeetingModeState>()
            .map(|m| m.capture_enabled())
            .unwrap_or(false);
        if !is_meeting_capture {
            recovery::begin();
        }
        let session = app
            .try_state::<RecordingSessionState>()
            .and_then(|s| s.current());
        let recording_id = session
            .as_ref()
            .map(|(_, id)| id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        if !is_meeting_capture {
            if !dictation_stt::vad_installed() {
                tracing::warn!(
                    "[speech] Silero VAD missing — downloading; local speech runs without VAD until install completes"
                );
                let app_vad = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = meeting_engine::meeting_download_silero_vad_model(app_vad).await
                    {
                        tracing::warn!("[stt] Silero VAD download failed: {e}");
                    }
                });
            }
            let live_nemotron = session
                .as_ref()
                .and_then(|(generation, _)| live_nemotron_controller(app, *generation));
            tracing::info!(
                provider = dictation_stt::provider_name(),
                live_websocket = live_nemotron.is_some(),
                "[speech] dictation audio drain started"
            );
            let drain_done_rx = whisper_dictation_stream::spawn_dictation_audio_drain(
                recording_id,
                chunk_recv,
                live_nemotron,
            );
            if let Some((generation, _)) = session {
                attach_live_nemotron_drain(app, generation, drain_done_rx);
            }
            return;
        }
    } else {
        tracing::debug!("[speech] no chunk receiver — local recovery tap not started");
    }
}

/// Stop the current recorder without polishing or emitting transcript.
/// Used for meeting-mode mute so Fn never acts like "send this chunk".
fn do_cancel_recording(
    shared: Arc<Mutex<DesktopApp>>,
    app: tauri::AppHandle,
    reason: &'static str,
) {
    LAST_FINISH_MS.store(now_ms_desktop(), Ordering::SeqCst);
    reset_long_dictation_lock(&app);
    restore_speaker_suppression(&app, reason);
    recovery::clear();
    discard_live_nemotron_run(&app);
    PROBLEM_START_PENDING.store(false, Ordering::SeqCst);

    if let Ok(mut route) = app.state::<RecordingRouteState>().0.lock() {
        *route = None;
    }

    let (stop_rx, snap) = {
        let mut d = match shared.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if d.state != desktop::AppState::Recording {
            return;
        }
        let stop_rx = match d.begin_stop() {
            Ok((stop_rx, _)) => Some(stop_rx),
            Err(e) => {
                tracing::warn!("[meeting_mode] cancel failed ({reason}): {e}");
                None
            }
        };
        let snap = d.finish_cancelled();
        tracing::info!("[meeting_mode] recording cancelled — reason={reason}");
        (stop_rx, snap)
    };

    // Emit the idle snapshot FIRST. The webview clears its "polishing"/"recording"
    // banner only on an idle app-state event, so this must reach the UI even if a
    // later call (tray/status) fails — a missed idle event is exactly what strands
    // the banner and the menu-bar mic indicator after a quick-tap cancel.
    emit_app_state(&app, &snap);
    sync_tray(&app, &snap);
    emit_meeting_stt_status(&app);

    if let Some(stop_rx) = stop_rx {
        std::thread::spawn(move || {
            let _ = stop_rx.recv();
        });
    }
}

/// Stop recording, ship WAV to backend via SSE, paste the result.
/// In meeting mode (`is_meeting`), skips pasting and emits `meeting-transcript` events,
/// then auto-restarts recording for the next utterance.
fn do_finish_recording(
    shared: Arc<Mutex<DesktopApp>>,
    app: tauri::AppHandle,
    back_arc: Arc<Mutex<Option<BackendEndpoint>>>,
) {
    diag::breadcrumb("record:finish:enter");
    FINISH_AFTER_START.store(false, Ordering::SeqCst);
    LAST_FINISH_MS.store(now_ms_desktop(), Ordering::SeqCst);
    clear_long_dictation_recording_lock(&app);

    let session_end = app
        .try_state::<RecordingSessionState>()
        .and_then(|session| session.end());
    let client_run_id = session_end.as_ref().map(|(_, id)| id.clone());
    // This run's recording generation. If the user starts a new recording while
    // this one is still processing (pressed record again to redo a mis-released
    // take), the generation bumps and this finish must not touch the shared state
    // machine — the newer recording owns it now.
    let finish_generation = session_end.as_ref().map(|(g, _)| *g);
    // Capture the live session before recorder shutdown. Its bridge owns a
    // controller clone until the mic stream closes; this receiver is the only
    // place that may commit and await the final transcript.
    let live_nemotron = take_live_nemotron_run(&app, finish_generation);
    let session_tag = session_end
        .as_ref()
        .map(|(generation, id)| format!("id={id} generation={generation}"))
        .unwrap_or_else(|| "id=unknown".into());

    let edit_target_pid = app
        .state::<EditTargetState>()
        .0
        .lock()
        .ok()
        .and_then(|mut target| target.take());

    enum BeginStopError {
        Short(AppSnapshot),
        Failed(AppSnapshot),
    }

    // Signal the recorder to stop while holding the app mutex, then wait for
    // samples and encode WAV after releasing it. Keep all UI/AppKit work outside
    // this mutex; tray/menu calls can block on macOS and must not freeze hotkeys.
    diag::breadcrumb("finish:enter");
    maybe_pad_dictation_release(&app);
    let begin_stop = {
        let mut d = match lock_shared(&shared, "finish_recording") {
            Ok(g) => g,
            Err(_) => {
                diag::breadcrumb("finish:lock_failed");
                restore_speaker_suppression(&app, "finish lock failed");
                return;
            }
        };
        match d.begin_stop() {
            Ok((stop_rx, was_too_short)) => {
                let snap = d.snapshot();
                tracing::info!("[record] stop initiated — state={}", snap.state);
                Ok((stop_rx, was_too_short, snap))
            }
            Err(e) => {
                if e == "not recording" && long_dictation_finishing(&app) {
                    diag::breadcrumb("record:finish:duplicate_long_finish");
                    tracing::debug!(
                        "[record] duplicate finish ignored while long dictation is already finishing"
                    );
                    return;
                }
                if is_short_recording_cancel(&e) {
                    diag::breadcrumb("record:finish:short_cancel");
                    tracing::info!("[record] short Option tap — cancelled recording");
                    let snap = d.finish_cancelled();
                    Err(BeginStopError::Short(snap))
                } else {
                    diag::breadcrumb("record:finish:begin_stop_failed");
                    let snap = d.finish_err(e);
                    Err(BeginStopError::Failed(snap))
                }
            }
        }
    };
    restore_speaker_suppression(&app, "finish initiated");

    let (stop_rx, was_too_short) = match begin_stop {
        Ok((stop_rx, was_too_short, snap)) => {
            sync_tray(&app, &snap);
            emit_app_state(&app, &snap);
            (stop_rx, was_too_short)
        }
        Err(BeginStopError::Short(snap)) => {
            recovery::clear();
            sync_tray(&app, &snap);
            emit_short_recording_error(&app);
            emit_app_state(&app, &snap);
            return;
        }
        Err(BeginStopError::Failed(snap)) => {
            recovery::clear();
            sync_tray(&app, &snap);
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": snap.last_error.clone().unwrap_or_else(|| "Recording failed".to_string()),
                    "audio_id": null,
                }),
            );
            emit_app_state(&app, &snap);
            return;
        }
    };

    let recording_route = app
        .state::<RecordingRouteState>()
        .0
        .lock()
        .ok()
        .and_then(|mut route| route.take())
        .unwrap_or(RecordingRoute::Normal);
    let is_meeting = recording_route == RecordingRoute::Meeting;
    let is_divo = recording_route == RecordingRoute::Divo;
    let is_problem = recording_route == RecordingRoute::Problem;
    let meeting_generation_at_stop = if is_meeting {
        app.try_state::<MeetingModeState>()
            .map(|s| s.generation.load(Ordering::SeqCst))
            .unwrap_or(0)
    } else {
        0
    };

    let wav = match desktop::DesktopApp::finish_stop(stop_rx, was_too_short) {
        Ok(wav) => wav,
        Err(e) => {
            let is_short = is_short_recording_cancel(&e);
            let snap = {
                let mut d = match shared.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                if is_short {
                    tracing::info!("[record] short Option tap — cancelled recording");
                    d.finish_cancelled()
                } else {
                    d.finish_err(e)
                }
            };
            recovery::clear();
            sync_tray(&app, &snap);
            if is_short {
                emit_short_recording_error(&app);
            } else {
                let _ = app.emit(
                    "voice-error",
                    serde_json::json!({
                        "message": snap.last_error.clone().unwrap_or_else(|| "Recording failed".to_string()),
                        "audio_id": null,
                    }),
                );
            }
            emit_app_state(&app, &snap);
            emit_meeting_stt_status(&app);
            return;
        }
    };

    // Do the async SSE pipeline in a tokio task
    let shared2 = Arc::clone(&shared);
    let app2 = app.clone();
    let back_arc2 = Arc::clone(&back_arc);

    tauri::async_runtime::spawn(async move {
        tracing::info!("[finish] pipeline start session={session_tag}");
        let stt_language = app2
            .try_state::<HotPathCache>()
            .and_then(|hot| hot.0.try_read().ok().map(|guard| guard.language.clone()))
            .filter(|lang| !lang.is_empty())
            .unwrap_or_else(|| "hi".to_string());

        let mut live_nemotron = live_nemotron;
        let pre_transcript = if let Some(levels) = dictation_wav_is_no_speech(&wav) {
            tracing::warn!(
                "[finish] no speech energy detected — suppressing transcription (peak={:.5}, rms={:.5}, samples={})",
                levels.peak,
                levels.rms,
                levels.samples,
            );
            // There is no final result to request for silence. Dropping this
            // session cancels it instead of submitting an empty utterance.
            drop(live_nemotron.take());
            Err("No speech detected — try speaking again.".to_string())
        } else if let Some(live_nemotron) = live_nemotron.take() {
            finish_live_nemotron_transcript(live_nemotron).await
        } else {
            stt_pre_transcript(&wav, &stt_language).await
        };

        // The provider's errors are complete, actionable sentences (offline,
        // missing model, rejected key, …) — surface them verbatim.
        let pre_transcript = match pre_transcript {
            Ok(t) => Some(t),
            Err(message) => {
                tracing::warn!("[finish] speech transcription failed: {message}");
                emit_voice_error_quiet(&app2, &message);
                let snap = {
                    let mut d = match shared2.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    d.finish_err(message)
                };
                recovery::clear();
                sync_tray(&app2, &snap);
                emit_app_state(&app2, &snap);
                emit_meeting_stt_status(&app2);
                clear_long_dictation_finishing(&app2, "no_pre_transcript");
                return;
            }
        };

        let screen_context = app2
            .try_state::<ScreenContextState>()
            .and_then(|s| s.0.lock().ok()?.clone());

        if is_problem {
            let result = run_problem_command(
                &back_arc2,
                wav,
                client_run_id.clone(),
                pre_transcript,
                screen_context,
                &app2,
                edit_target_pid,
            )
            .await;

            let (snap, err_msg) = {
                let mut d = match shared2.lock() {
                    Ok(g) => g,
                    Err(_) => {
                        emit_voice_error_quiet(&app2, "Recording interrupted");
                        recovery::clear();
                        return;
                    }
                };
                match result {
                    Ok(ProblemCommandOutcome::Completed(done)) => (
                        d.finish_ok(ProcessSummary {
                            transcript: done.transcript,
                            polished: done.response.output,
                            model: done.response.model_used,
                            confidence: if done.pasted { 1.0 } else { 0.0 },
                            transcribe_ms: done.transcribe_ms.max(0) as u64,
                            polish_ms: done.response.latency_ms.total.max(0) as u64,
                        }),
                        None,
                    ),
                    Ok(ProblemCommandOutcome::Ambiguous { transcript }) => {
                        tracing::info!(
                            "[problem] ambiguous project match — stopped before solve transcript_chars={}",
                            transcript.chars().count()
                        );
                        (d.finish_cancelled(), None)
                    }
                    Err(ref e) => (
                        d.finish_err(strip_voice_error_already_emitted(e).to_string()),
                        Some(e.clone()),
                    ),
                }
            };
            if let Some(e) = err_msg.filter(|e| !is_voice_error_already_emitted(e)) {
                emit_voice_error_quiet(&app2, &e);
            }
            sync_tray(&app2, &snap);
            emit_app_state(&app2, &snap);
            emit_meeting_stt_status(&app2);
            recovery::clear();
            clear_long_dictation_finishing(&app2, "problem_done");
            return;
        }

        // Resolve the locked target pid → app key (bundle-id on macOS, exe on
        // Windows) so History can render which app this was dictated into.
        // `Option<i32>` is Copy, so this doesn't disturb edit_target_pid's later use.
        let target_app = edit_target_pid.and_then(app_identity::app_key_for_pid);
        // Opt-in, on-device: if the target is a browser, capture the active
        // tab's site (host only) for the Insights "Sites" section. Fire-and-forget.
        maybe_capture_browser_site(&back_arc2, target_app.clone());
        let result = run_voice_polish_sse(
            &back_arc2,
            wav,
            target_app,
            client_run_id.clone(),
            pre_transcript,
            None,
            screen_context,
            None,
            &app2,
            is_meeting,
            is_divo,
        )
        .await;

        // A newer recording superseded this run (user pressed record again during
        // processing to redo a mis-released take). The new recording now owns the
        // state machine and recovery buffer — this stale finish must not call
        // finish_ok/err (which would reset the live recording to Idle) or paste.
        // Its paste was already suppressed inside run_voice_polish_sse.
        if app2
            .try_state::<RecordingSessionState>()
            .map(|s| Some(s.generation.load(Ordering::SeqCst)) != finish_generation)
            .unwrap_or(false)
        {
            tracing::info!(
                "[record] finish superseded by a newer recording — leaving the live run intact (run_id={})",
                client_run_id.as_deref().unwrap_or("none"),
            );
            clear_long_dictation_finishing(&app2, "superseded_finish");
            return;
        }

        if is_divo {
            // Divo turn: reset the desktop recording state (capture is done), then
            // hand the polished instruction to the Divo bridge. The Divo SSE drives
            // the HUD from here via `divo-*` events — no paste, no edit-watcher.
            let (snap, instruction, err) = {
                let mut d = match shared2.lock() {
                    Ok(g) => g,
                    Err(_) => {
                        emit_voice_error_quiet(&app2, "Recording interrupted");
                        recovery::clear();
                        return;
                    }
                };
                match result {
                    Ok(done) => {
                        let text = done.polished.clone();
                        (
                            d.finish_ok(ProcessSummary {
                                transcript: done.transcript.clone(),
                                polished: done.polished,
                                model: done.model_used,
                                confidence: done.confidence.unwrap_or(0.0),
                                transcribe_ms: done.latency_ms.transcribe as u64,
                                polish_ms: done.latency_ms.polish as u64,
                            }),
                            Some(text),
                            None,
                        )
                    }
                    Err(ref e) => (
                        d.finish_err(strip_voice_error_already_emitted(e).to_string()),
                        None,
                        Some(e.clone()),
                    ),
                }
            };
            sync_tray(&app2, &snap);
            emit_app_state(&app2, &snap);

            // Default routing for the staged turn: Ctrl+N forces a new chat;
            // otherwise we continue the active thread if there is one (the HUD lets
            // the user override either way via the chat router).
            let new_chat = DIVO_NEW_CHAT_PENDING.swap(false, Ordering::SeqCst);
            let _ = DIVO_FOLLOWUP_PENDING.swap(false, Ordering::SeqCst);
            let current_thread = app2.state::<divo::DivoState>().current_thread();
            match (instruction, err) {
                (Some(text), _) if !text.trim().is_empty() => {
                    // Don't auto-send. Stage the polished transcript so the user
                    // can review/edit it, pick a target chat, and press Send (or
                    // cancel). The Send button invokes `divo_send`.
                    tracing::info!(
                        "[divo] staging instruction for review ({} chars, new_chat={new_chat}, current_thread={:?})",
                        text.len(),
                        current_thread.as_deref()
                    );
                    let _ = app2.emit(
                        "divo-stage",
                        serde_json::json!({
                            "text": text,
                            "newChat": new_chat,
                            "currentThreadId": current_thread,
                        }),
                    );
                }
                (Some(_), _) => {
                    // Empty transcript (e.g. a stray Ctrl tap with no speech) — don't
                    // bother Divo, and keep the HUD quiet.
                    tracing::info!("[divo] empty instruction — not sending to Divo");
                }
                (None, Some(e)) if !is_voice_error_already_emitted(&e) => {
                    tracing::warn!("[divo] transcription failed before send: {e}");
                    let _ = app2.emit(
                        "divo-error",
                        serde_json::json!({ "message": humanize_error(&e) }),
                    );
                }
                (None, Some(e)) => {
                    tracing::debug!(
                        "[divo] transcription failure already reported through voice-error: {}",
                        strip_voice_error_already_emitted(&e)
                    );
                }
                _ => {}
            }
            recovery::clear();
            clear_long_dictation_finishing(&app2, "divo_done");
            return;
        }

        if is_meeting {
            // Meeting mode: emit polished text as meeting-transcript, skip edit-watcher
            let meeting_still_valid = app2
                .try_state::<MeetingModeState>()
                .map(|s| {
                    s.capture_enabled()
                        && s.generation.load(Ordering::SeqCst) == meeting_generation_at_stop
                })
                .unwrap_or(false);
            if let Ok(ref done) = result {
                if meeting_still_valid && !done.polished.is_empty() {
                    let timestamp_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    tracing::info!(
                        "[meeting_mode] emitting polished transcript: {:?}",
                        done.polished.chars().take(80).collect::<String>()
                    );
                    let _ = app2.emit(
                        "meeting-transcript",
                        serde_json::json!({
                            "text": done.polished,
                            "timestamp_ms": timestamp_ms,
                        }),
                    );
                } else if !meeting_still_valid {
                    tracing::info!("[meeting_mode] discarding stale meeting chunk after mute/exit");
                }
            }

            let (snap, err_msg) = {
                let mut d = match shared2.lock() {
                    Ok(g) => g,
                    Err(_) => {
                        emit_voice_error_quiet(&app2, "Recording interrupted");
                        return;
                    }
                };
                match result {
                    Ok(done) => (
                        d.finish_ok(ProcessSummary {
                            transcript: done.transcript.clone(),
                            polished: done.polished,
                            model: done.model_used,
                            confidence: done.confidence.unwrap_or(0.0),
                            transcribe_ms: done.latency_ms.transcribe as u64,
                            polish_ms: done.latency_ms.polish as u64,
                        }),
                        None,
                    ),
                    Err(ref e) => (
                        d.finish_err(strip_voice_error_already_emitted(e).to_string()),
                        Some(e.clone()),
                    ),
                }
            };
            if let Some(e) = err_msg.filter(|e| !is_voice_error_already_emitted(e)) {
                emit_voice_error_quiet(&app2, &e);
            }
            sync_tray(&app2, &snap);
            emit_app_state(&app2, &snap);
            emit_meeting_stt_status(&app2);

            // Auto-restart recording for continuous meeting capture
            let still_meeting = app2
                .try_state::<MeetingModeState>()
                .map(|s| {
                    s.capture_enabled()
                        && s.generation.load(Ordering::SeqCst) == meeting_generation_at_stop
                })
                .unwrap_or(false);
            if still_meeting {
                let shared3 = Arc::clone(&shared2);
                let app3 = app2.clone();
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                std::thread::spawn(move || {
                    do_start_recording(&shared3, &app3);
                });
            }
            clear_long_dictation_finishing(&app2, "meeting_done");
        } else {
            // Normal mode: paste and edit-watch
            // Spawn edit-watcher immediately after paste (non-blocking).
            // Capture watch_start NOW — before the spawn — so the ring
            // buffer timestamp filter doesn't miss early mouse clicks.
            let watch_start = std::time::Instant::now();
            if let Ok(ref done) = result {
                let back3 = Arc::clone(&back_arc2);
                let pre_paste = app2
                    .try_state::<ScreenContextState>()
                    .and_then(|s| s.0.lock().ok()?.clone());
                let anchor = capture_edit_anchor(edit_target_pid, pre_paste, &done.polished);
                start_edit_watcher(
                    back3,
                    app2.clone(),
                    client_run_id.clone(),
                    done.recording_id.clone(),
                    done.polished.clone(),
                    watch_start,
                    anchor,
                );
            }

            let (snap, err_msg) = {
                let mut d = match shared2.lock() {
                    Ok(g) => g,
                    Err(_) => {
                        emit_voice_error_quiet(&app2, "Recording interrupted");
                        return;
                    }
                };
                match result {
                    Ok(done) => (
                        d.finish_ok(ProcessSummary {
                            transcript: done.transcript.clone(),
                            polished: done.polished,
                            model: done.model_used,
                            confidence: done.confidence.unwrap_or(0.0),
                            transcribe_ms: done.latency_ms.transcribe as u64,
                            polish_ms: done.latency_ms.polish as u64,
                        }),
                        None,
                    ),
                    Err(ref e) => (
                        d.finish_err(strip_voice_error_already_emitted(e).to_string()),
                        Some(e.clone()),
                    ),
                }
            };
            if let Some(e) = err_msg.filter(|e| !is_voice_error_already_emitted(e)) {
                emit_voice_error_quiet(&app2, &e);
            }
            sync_tray(&app2, &snap);
            emit_app_state(&app2, &snap);
            emit_meeting_stt_status(&app2);
            clear_long_dictation_finishing(&app2, "normal_done");
        }
        // Dictation delivered (or surfaced as an error to the user) — the captured
        // audio is no longer needed, so the orphan file must not linger.
        recovery::clear();
    });
}

fn reject_pre_transcript_reason(
    transcript: &str,
    word_count: usize,
    wav_duration_s: f64,
) -> Option<String> {
    let trimmed = transcript.trim();
    if trimmed.contains("<|") || trimmed.contains("|>") {
        return Some("contains Whisper control tokens".to_string());
    }
    let compact: String = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if compact.is_empty() {
        return Some("empty transcript".to_string());
    }
    if !compact.chars().any(|c| c.is_alphanumeric()) {
        return Some("no alphanumeric speech tokens".to_string());
    }
    let punct = compact.chars().filter(|c| !c.is_alphanumeric()).count();
    if compact.chars().count() >= 20 && punct as f64 / compact.chars().count() as f64 > 0.65 {
        return Some("mostly punctuation".to_string());
    }
    let normalized: Vec<String> = trimmed
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|c: char| {
                    !c.is_alphanumeric() && !('\u{0900}'..='\u{097F}').contains(&c)
                })
                .to_ascii_lowercase()
        })
        .filter(|token| !token.is_empty())
        .collect();
    if normalized.len() >= 8 {
        let mut counts = std::collections::HashMap::<&str, usize>::new();
        for token in &normalized {
            *counts.entry(token.as_str()).or_insert(0) += 1;
        }
        let top = counts.values().copied().max().unwrap_or(0);
        let unique_ratio = counts.len() as f64 / normalized.len() as f64;
        let top_ratio = top as f64 / normalized.len() as f64;
        let avg_len = normalized
            .iter()
            .map(|token| token.chars().count())
            .sum::<usize>() as f64
            / normalized.len() as f64;
        if top_ratio >= 0.55 && unique_ratio <= 0.30 {
            return Some("repetition loop".to_string());
        }
        if avg_len <= 1.25 && normalized.len() >= 12 {
            return Some("single-character token loop".to_string());
        }
    }
    if wav_duration_s > 2.0 && word_count > (wav_duration_s * 18.0).ceil() as usize {
        return Some("implausible word rate".to_string());
    }
    None
}

#[derive(Debug, Clone, Copy)]
struct DictationAudioLevels {
    peak: f64,
    rms: f64,
    samples: usize,
}

fn dictation_pcm16_levels(wav: &[u8]) -> Option<DictationAudioLevels> {
    if wav.len() < 44 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    while offset + 8 <= wav.len() {
        let chunk_id = &wav[offset..offset + 4];
        let chunk_len = u32::from_le_bytes([
            wav[offset + 4],
            wav[offset + 5],
            wav[offset + 6],
            wav[offset + 7],
        ]) as usize;
        offset += 8;
        if offset + chunk_len > wav.len() {
            return None;
        }
        if chunk_id == b"data" {
            let data = &wav[offset..offset + chunk_len];
            let mut samples = 0usize;
            let mut peak = 0f64;
            let mut sum_sq = 0f64;
            for bytes in data.chunks_exact(2) {
                let sample = i16::from_le_bytes([bytes[0], bytes[1]]) as f64 / i16::MAX as f64;
                let amp = sample.abs();
                peak = peak.max(amp);
                sum_sq += sample * sample;
                samples += 1;
            }
            if samples == 0 {
                return None;
            }
            return Some(DictationAudioLevels {
                peak,
                rms: (sum_sq / samples as f64).sqrt(),
                samples,
            });
        }
        offset += chunk_len + (chunk_len % 2);
    }
    None
}

fn dictation_wav_is_no_speech(wav: &[u8]) -> Option<DictationAudioLevels> {
    let levels = dictation_pcm16_levels(wav)?;
    (levels.peak < DICTATION_NO_SPEECH_PEAK && levels.rms < DICTATION_NO_SPEECH_RMS)
        .then_some(levels)
}

/// Re-transcribe a recovered orphan recording. Uses a no-op event handler so the
/// pipeline performs zero typing/pasting — recovery must never inject text into
/// whatever app happens to be focused at launch. Returns transcript + polished text.
async fn recover_transcribe(ep: &BackendEndpoint, wav: Vec<u8>) -> Result<api::PolishDone, String> {
    api::stream_voice_polish(
        ep,
        wav,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        false,
        |_event: api::PolishEvent| {},
    )
    .await
}

struct ProblemCommandCompleted {
    transcript: String,
    response: api::ProblemSolveResponse,
    pasted: bool,
    transcribe_ms: i64,
}

enum ProblemCommandOutcome {
    Completed(ProblemCommandCompleted),
    Ambiguous { transcript: String },
}

async fn run_problem_command(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    wav: Vec<u8>,
    client_run_id: Option<String>,
    pre_transcript: Option<dictation_stt::PreTranscript>,
    screen_context: Option<String>,
    app: &tauri::AppHandle,
    edit_target_pid: Option<i32>,
) -> Result<ProblemCommandOutcome, String> {
    let ep = {
        let lock = back_arc.lock().map_err(|_| "backend lock failed")?;
        lock.clone().ok_or("backend not started")?
    };

    emit_voice_status(app, "problem_transcribing", None, client_run_id.as_deref());
    let pre_text = pre_transcript.as_ref().map(|t| t.transcript.clone());
    let pre_meta = pre_transcript.as_ref().map(|t| t.meta.clone());
    let transcribed =
        api::transcribe_problem_audio(&ep, wav, client_run_id.clone(), pre_text, pre_meta).await?;
    let transcript = transcribed.transcript.trim().to_string();
    if transcript.is_empty() {
        return Err("No speech detected — try speaking again".to_string());
    }

    let settings = developer_context::load_settings();
    let context_match = if settings.enabled {
        developer_context::match_transcript(&transcript, &settings)
    } else {
        developer_context::DeveloperContextMatch {
            outcome: "none".to_string(),
            label: "No Project Context".to_string(),
            project: None,
            candidates: Vec::new(),
        }
    };
    emit_problem_context_event(app, &context_match);

    match context_match.outcome.as_str() {
        "ambiguous" => {
            let event_run_id = client_run_id.clone();
            if let Some(pending) = app.try_state::<PendingProblemState>() {
                if let Ok(mut slot) = pending.0.lock() {
                    *slot = Some(PendingProblemCommand {
                        transcript: transcript.clone(),
                        screen_context,
                        client_run_id,
                        edit_target_pid,
                        candidates: context_match.candidates,
                        created_at_ms: now_ms_desktop() as i64,
                    });
                }
            }
            let _ = app.emit(
                "voice-output",
                serde_json::json!({
                    "status": "manual_paste",
                    "message": "Ambiguous Project Match",
                    "run_id": event_run_id.as_deref(),
                }),
            );
            Ok(ProblemCommandOutcome::Ambiguous { transcript })
        }
        "project" => {
            let (response, pasted) = solve_problem_with_project(
                back_arc,
                app,
                transcript.clone(),
                screen_context,
                client_run_id,
                edit_target_pid,
                context_match.project,
            )
            .await?;
            Ok(ProblemCommandOutcome::Completed(ProblemCommandCompleted {
                transcript,
                response,
                pasted,
                transcribe_ms: transcribed.latency_ms,
            }))
        }
        _ => {
            let (response, pasted) = solve_problem_with_project(
                back_arc,
                app,
                transcript.clone(),
                screen_context,
                client_run_id,
                edit_target_pid,
                None,
            )
            .await?;
            Ok(ProblemCommandOutcome::Completed(ProblemCommandCompleted {
                transcript,
                response,
                pasted,
                transcribe_ms: transcribed.latency_ms,
            }))
        }
    }
}

async fn solve_problem_with_project(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    app: &tauri::AppHandle,
    transcript: String,
    screen_context: Option<String>,
    client_run_id: Option<String>,
    edit_target_pid: Option<i32>,
    project: Option<developer_context::DeveloperMatchedProject>,
) -> Result<(api::ProblemSolveResponse, bool), String> {
    let ep = {
        let lock = back_arc.lock().map_err(|_| "backend lock failed")?;
        lock.clone().ok_or("backend not started")?
    };

    emit_voice_status(
        app,
        "problem_solving",
        Some(transcript.as_str()),
        client_run_id.as_deref(),
    );

    let context_mode = if project.is_some() {
        "project".to_string()
    } else {
        "generic".to_string()
    };
    let req = api::ProblemSolveRequest {
        transcript,
        context_mode,
        project_id: project.as_ref().map(|p| p.id.clone()),
        project_name: project.as_ref().map(|p| p.name.clone()),
        project_context: project.as_ref().map(|p| p.context.clone()),
        screen_context,
        client_run_id,
        platform: Some(std::env::consts::OS.to_string()),
        app_version: option_env!("CARGO_PKG_VERSION").map(str::to_string),
    };

    let event_run_id = req.client_run_id.clone();
    let response = api::solve_problem(&ep, req).await?;
    let pasted = paste_problem_output(app, &response.output, edit_target_pid).await;
    let _ = app.emit(
        "voice-output",
        serde_json::json!({
            "status": if pasted { "pasted" } else { "manual_paste" },
            "message": if pasted { "Pasted" } else { "Ready to Paste" },
            "run_id": event_run_id.as_deref(),
        }),
    );
    Ok((response, pasted))
}

async fn paste_problem_output(
    app: &tauri::AppHandle,
    output: &str,
    edit_target_pid: Option<i32>,
) -> bool {
    if output.trim().is_empty() {
        return false;
    }

    if let Some(expected_pid) = edit_target_pid {
        let current_pid = blocking_ax_option("problem focused_pid", paster::focused_pid).await;
        if current_pid != Some(expected_pid) {
            tracing::warn!(
                "[problem] paste deferred — focus changed expected_pid={:?} current_pid={:?}",
                edit_target_pid,
                current_pid,
            );
            let _ = app.emit(
                "problem-command-ready-to-paste",
                serde_json::json!({"chars": output.chars().count()}),
            );
            return false;
        }
    }

    let (insert_res, used_clipboard) = insert_text_prefer_direct("problem_final_insert", output);
    match insert_res {
        Ok(()) => {
            tracing::info!(
                "[problem] final output inserted chars={} clipboard_fallback={}",
                output.chars().count(),
                used_clipboard,
            );
            true
        }
        Err(err) => {
            tracing::warn!("[problem] final insert failed: {err}");
            false
        }
    }
}

fn emit_problem_context_event(
    app: &tauri::AppHandle,
    context_match: &developer_context::DeveloperContextMatch,
) {
    let candidate_json = context_match
        .candidates
        .iter()
        .map(|candidate| {
            serde_json::json!({
                "id": candidate.id,
                "name": candidate.name,
                "matched_alias": candidate.matched_alias,
            })
        })
        .collect::<Vec<_>>();
    let project_json = context_match.project.as_ref().map(|project| {
        serde_json::json!({
            "id": project.id,
            "name": project.name,
            "matched_alias": project.matched_alias,
        })
    });
    let _ = app.emit(
        "problem-command-context",
        serde_json::json!({
            "outcome": context_match.outcome,
            "label": context_match.label,
            "project": project_json,
            "candidates": candidate_json,
        }),
    );
}

/// Async SSE consumer: previews backend tokens, inserts the final result once,
/// and stores it for paste-latest re-paste.
/// Opt-in browser-context capture. If `target_app` is a browser and the
/// `browser_context_enabled` pref is set, read the active tab's site (host only)
/// and record it to the LOCAL backend. Fully fire-and-forget and on-device — the
/// host is never sent to the cloud runtime. No-op when disabled / not a browser.
fn maybe_capture_browser_site(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    target_app: Option<String>,
) {
    let Some(app_key) = target_app else {
        return;
    };
    if !browser_context::is_known_browser(&app_key) {
        return;
    }
    if !said_core::prefs::load().browser_context_enabled {
        return;
    }
    let back_arc = back_arc.clone();
    tauri::async_runtime::spawn(async move {
        // osascript is blocking — keep it off the async runtime thread.
        let key = app_key.clone();
        let site = tauri::async_runtime::spawn_blocking(move || browser_context::active_site(&key))
            .await
            .ok()
            .flatten();
        let Some(site) = site else {
            return;
        };
        let ep = back_arc.lock().ok().and_then(|g| g.clone());
        if let Some(ep) = ep {
            let _ = api::record_site_context(&ep, &app_key, &site.host).await;
        }
    });
}

async fn run_voice_polish_sse(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    wav: Vec<u8>,
    target_app: Option<String>,
    client_run_id: Option<String>,
    pre_transcript: Option<dictation_stt::PreTranscript>,
    repair_mode: Option<String>,
    screen_context: Option<String>,
    message_polish_override: Option<bool>,
    app: &tauri::AppHandle,
    #[allow(unused_variables)] is_meeting: bool,
    is_divo: bool,
) -> Result<api::PolishDone, String> {
    let ep = {
        let lock = back_arc.lock().map_err(|_| "backend lock failed")?;
        lock.clone().ok_or("backend not started")?
    };

    // Meeting capture AND Divo turns both suppress all local output (no live
    // typing, no paste, no focused-field read) — they only need the polished text.
    let suppress_local = is_meeting || is_divo;
    let app_clone = app.clone();
    // Snapshot this run's recording generation. If the user starts a NEW
    // recording while this one is still processing (e.g. they released the
    // hotkey early by mistake and pressed it again to redo), the generation
    // bumps — and this now-superseded run must stop typing and skip its paste so
    // the abandoned take never gets injected into the focused app.
    let run_generation = app
        .try_state::<RecordingSessionState>()
        .map(|s| s.generation.load(Ordering::SeqCst))
        .unwrap_or(0);
    let message_polish_mode = !suppress_local
        && message_polish_override.unwrap_or_else(|| said_core::prefs::load().message_polish_mode);
    tracing::debug!(
        "[pipeline] target typing waits for final server output; stream tokens are preview-only"
    );
    let initial_field_read_t0 = std::time::Instant::now();
    let initial_field_text = if suppress_local {
        None
    } else {
        paster::read_focused_value_fast()
    };
    if !suppress_local {
        if let Ok(mut context) = app.state::<ScreenContextState>().0.lock() {
            *context = initial_field_text
                .clone()
                .filter(|text| !text.trim().is_empty());
        }
    }
    let initial_field_read_ms = initial_field_read_t0.elapsed().as_millis() as i64;
    let error_target_app = target_app.clone();
    let error_client_run_id = client_run_id.clone();
    let event_client_run_id = client_run_id.clone();
    let error_already_emitted = Arc::new(AtomicBool::new(false));
    let error_already_emitted_for_events = Arc::clone(&error_already_emitted);

    let pre_transcript_chars = pre_transcript
        .as_ref()
        .map(|t| t.transcript.chars().count())
        .unwrap_or(0);
    let pre_transcript_words = pre_transcript
        .as_ref()
        .map(|t| t.transcript.split_whitespace().count())
        .unwrap_or(0);
    let pre_transcript_meta = pre_transcript.as_ref().map(|t| {
        format!(
            "confidence={:.2} mean_word_confidence={:.2} meta_words={} model={} origin={:?}",
            t.meta.confidence,
            t.meta.mean_word_confidence,
            t.meta.word_count,
            t.meta.model,
            t.meta.origin,
        )
    });
    let pre_transcript_duration_ms = pre_transcript
        .as_ref()
        .map(|t| t.meta.duration_ms)
        .unwrap_or(0);
    let pipeline_path = "local_transcript";
    tracing::info!(
        "[pipeline] backend handoff run_id={} path={} wav_bytes={} pre_transcript_present={} pre_chars={} pre_words={} message_polish={} repair_mode={} screen_context_chars={} meta={}",
        client_run_id.as_deref().unwrap_or("none"),
        pipeline_path,
        wav.len(),
        pre_transcript.is_some(),
        pre_transcript_chars,
        pre_transcript_words,
        message_polish_mode,
        repair_mode.is_some(),
        screen_context
            .as_ref()
            .map(|s| s.chars().count())
            .unwrap_or(0),
        pre_transcript_meta.as_deref().unwrap_or("none"),
    );
    tracing::info!(
        "[pipeline] → sending to backend: wav={}KB pre_transcript={}",
        wav.len() / 1024,
        pre_transcript
            .as_ref()
            .map(|t| {
                let truncated: String = t.transcript.chars().take(80).collect();
                if truncated.len() < t.transcript.len() {
                    format!("\"{truncated}…\"")
                } else {
                    format!("\"{}\"", t.transcript)
                }
            })
            .unwrap_or_else(|| "none (local speech failed)".into()),
    );
    let mut on_polish_event = move |event| {
        match &event {
            api::PolishEvent::Token { token } => {
                // A newer recording superseded this run (user pressed the hotkey
                // again to redo a mis-released take) — stop previewing its tokens.
                if app_clone
                    .try_state::<RecordingSessionState>()
                    .map(|s| s.generation.load(Ordering::SeqCst) != run_generation)
                    .unwrap_or(false)
                {
                    return;
                }
                // The backend uses this internal marker to invalidate an unsafe
                // partial stream. It is never user-visible and final insertion
                // does not need reconciliation.
                if token != STREAM_RESET_SENTINEL {
                    let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
                }
            }
            api::PolishEvent::Status { phase, transcript } => {
                if app_clone
                    .try_state::<RecordingSessionState>()
                    .map(|s| s.generation.load(Ordering::SeqCst) != run_generation)
                    .unwrap_or(false)
                {
                    return;
                }
                tracing::info!(
                    "[pipeline] status: phase={phase} transcript={}",
                    transcript.as_deref().unwrap_or("—")
                );
                emit_voice_status(
                    &app_clone,
                    phase,
                    transcript.as_deref(),
                    event_client_run_id.as_deref(),
                );
            }
            api::PolishEvent::Done(done) => {
                if app_clone
                    .try_state::<RecordingSessionState>()
                    .map(|s| s.generation.load(Ordering::SeqCst) != run_generation)
                    .unwrap_or(false)
                {
                    return;
                }
                tracing::info!(
                    "[pipeline] ✓ done: {} chars, model={}, latency: stt={}ms embed={}ms polish={}ms total={}ms",
                    done.polished.len(),
                    done.model_used,
                    done.latency_ms.transcribe,
                    done.latency_ms.embed,
                    done.latency_ms.polish,
                    done.latency_ms.total,
                );
                // Diagnostic — logs the full polished text (capped) so we can
                // tell the difference between an LLM that produced duplicate
                // text vs a typing-path bug that doubled it. Caps at 400
                // chars so very long polishes don't blow up the log.
                let preview: String = done.polished.chars().take(400).collect();
                let suffix = if done.polished.chars().count() > 400 {
                    "…"
                } else {
                    ""
                };
                tracing::info!("[pipeline] polished text: \"{preview}{suffix}\"");
                // For Divo turns the Divo bridge drives the HUD — don't flash "Done".
                if !is_divo {
                    emit_voice_done(&app_clone, done, event_client_run_id.as_deref());
                }
            }
            api::PolishEvent::Error {
                message,
                run_id,
                audio_id,
                error_code,
                retryable,
                owned_by_airnote,
                diagnostic,
            } => {
                error_already_emitted_for_events.store(true, Ordering::Relaxed);
                tracing::error!("[pipeline] backend error: {message}");
                cache_last_failed_voice_action(
                    &app_clone,
                    audio_id.clone(),
                    diagnostic.as_deref().unwrap_or(message),
                    error_code.clone(),
                    error_target_app.clone(),
                    run_id.clone().or_else(|| error_client_run_id.clone()),
                    message_polish_mode,
                );
                let human = humanize_error(&message);
                if error_code.as_deref() == Some("together_rate_limit") {
                    notify_macos(&app_clone, "Together AI rate limit hit", &human);
                }
                let _ = app_clone.emit(
                    "voice-error",
                    serde_json::json!({
                        "message":  human,
                        "raw_error": message,
                        "run_id": run_id,
                        "audio_id": audio_id,
                        "error_code": error_code,
                        "retryable": retryable,
                        "owned_by_airnote": owned_by_airnote,
                        "diagnostic": diagnostic,
                        "auto_hide_ms": 4000,
                    }),
                );
            }
        }
    };

    let wav_len = wav.len();
    let target_app_for_telemetry = target_app.clone();
    let mut initial_trace = said_core::dictation_trace::DictationTrace::default();
    initial_trace.add_stage(said_core::dictation_trace::TraceStageInput {
        stage: "desktop.pipeline.handoff",
        component: "desktop",
        function: "run_voice_polish_sse",
        output: pre_transcript.as_ref().map(|t| t.transcript.as_str()),
        reason: Some("desktop handing audio/transcript candidate to backend"),
        risk: Some("desktop_transcript_selection"),
        metadata: serde_json::json!({
            "run_id": client_run_id.as_deref(),
            "path": pipeline_path,
            "wav_bytes": wav_len,
            "pre_transcript_present": pre_transcript.is_some(),
            "pre_transcript_chars": pre_transcript_chars,
            "pre_transcript_words": pre_transcript_words,
            "local_asr_ms": pre_transcript_duration_ms,
            "message_polish": message_polish_mode,
            "repair_mode": repair_mode.is_some(),
            "screen_context_chars": screen_context.as_ref().map(|s| s.chars().count()).unwrap_or(0),
        }),
        ..Default::default()
    });
    initial_trace.add_stage(said_core::dictation_trace::TraceStageInput {
        stage: "desktop.field.initial_snapshot",
        component: "desktop",
        function: "paster::read_focused_value_fast",
        output: initial_field_text.as_deref(),
        duration_ms: Some(initial_field_read_ms),
        reason: Some("field text before live typing/reconcile"),
        risk: Some("paste_context"),
        metadata: serde_json::json!({
            "readable": initial_field_text.as_ref().is_some_and(|s| !s.is_empty()),
            "suppress_local": suppress_local,
        }),
        ..Default::default()
    });
    let initial_trace_json = Some(initial_trace.into_value());
    let Some(transcript) = pre_transcript else {
        return Err("Local transcript is required before voice polish".into());
    };
    let speech_identity = telemetry::speech_identity(&transcript.meta);
    tracing::info!("[pipeline] sending WAV + local transcript to backend");
    let done_result = api::stream_voice_polish(
        &ep,
        wav,
        target_app,
        client_run_id.clone(),
        initial_trace_json,
        Some(transcript.transcript),
        Some(transcript.meta),
        repair_mode,
        screen_context,
        message_polish_mode,
        &mut on_polish_event,
    )
    .await;
    let done = match done_result {
        Ok(d) => d,
        Err(e) => {
            if let Some(run_id) = client_run_id.as_deref() {
                telemetry::on_pipeline_error(&ep, run_id, None);
            }
            if error_already_emitted.load(Ordering::Relaxed) {
                return Err(mark_voice_error_already_emitted(&e));
            }
            let mut retry_audio_id: Option<String> = None;
            if let Some(run_id) = client_run_id.as_deref() {
                match api::mark_voice_run_failed(&ep, run_id, "sse_missing_done", &e, true, true)
                    .await
                {
                    Ok(Some(run)) => {
                        retry_audio_id = run.audio_id.clone();
                        tracing::warn!(
                            "[retry] marked voice run failed after missing done run_id={} audio_id={} status={} attempts={}",
                            run.run_id,
                            run.audio_id.as_deref().unwrap_or("none"),
                            run.status,
                            run.attempt_count,
                        );
                    }
                    Ok(None) => {
                        tracing::warn!(
                            "[retry] missing-done failure could not be linked to run_id={run_id}"
                        );
                    }
                    Err(mark_err) => {
                        tracing::warn!(
                            "[retry] failed to mark missing-done run_id={run_id}: {mark_err}"
                        );
                    }
                }
            }
            cache_last_failed_voice_action(
                &app,
                retry_audio_id.clone(),
                &e,
                Some("sse_missing_done".to_string()),
                target_app_for_telemetry.clone(),
                client_run_id.clone(),
                message_polish_mode,
            );
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": "Audio saved. Processing failed.",
                    "raw_error": e,
                    "run_id": client_run_id,
                    "audio_id": retry_audio_id,
                    "error_code": "sse_missing_done",
                    "retryable": true,
                    "owned_by_airnote": true,
                    "diagnostic": "SSE stream ended without a done event after audio capture",
                    "auto_hide_ms": 4000,
                }),
            );
            return Err(mark_voice_error_already_emitted(
                "SSE stream ended without a `done` event",
            ));
        }
    };

    let mut output_pasted = false;
    let mut used_clipboard_fallback = false;
    // If a newer recording superseded this one, never paste the abandoned take.
    let superseded = app
        .try_state::<RecordingSessionState>()
        .map(|s| s.generation.load(Ordering::SeqCst) != run_generation)
        .unwrap_or(false);
    if superseded {
        tracing::info!(
            "[main] run superseded by a newer recording — skipping paste run_id={}",
            client_run_id.as_deref().unwrap_or("none"),
        );
    } else if suppress_local {
        tracing::info!("[main] meeting/divo mode — skipping paste for polished chunk");
    } else {
        // Insert only the final server result so the focused field never needs
        // a second reconciliation mutation.
        tracing::info!(
            "[main] direct insert final server result ({} chars)",
            done.polished.len()
        );
        if !done.polished.is_empty() {
            let (insert_res, clipboard) =
                insert_text_prefer_direct("main_final_insert", &done.polished);
            used_clipboard_fallback = clipboard;
            match insert_res {
                Ok(()) => {
                    output_pasted = true;
                }
                Err(e) => {
                    tracing::warn!("[main] final insert failed: {e}");
                }
            }
        }
    }

    // Always store latest result so the paste-latest hotkey can re-paste it any time.
    // Divo instructions are commands, not dictation output — never store them.
    // A superseded run is abandoned — don't make its text the paste-latest target.
    if !is_divo && !superseded && !done.polished.is_empty() {
        if let Ok(mut g) = app.state::<LatestResult>().0.lock() {
            *g = Some(done.polished.clone());
        }
        cache_last_voice_action(app, &done, LastRepairStage::None);
        tracing::debug!(
            "[main] result stored ({} chars) — {} to paste again",
            done.polished.len(),
            paste_latest_hotkey_label()
        );
        // Diagnostic: log the actual polished text (capped at 240 chars for
        // privacy / log-volume reasons). This makes LLM-side regressions
        // (e.g. duplicate-output bugs from prompt drift) immediately visible
        // — without it we only see token counts and can't tell whether
        // duplication is in the model output or in our typing path.
        let preview: String = done.polished.chars().take(240).collect();
        let suffix = if done.polished.chars().count() > 240 {
            "…"
        } else {
            ""
        };
        tracing::debug!("[main] polished text: \"{preview}{suffix}\"");
    }

    let output_status = if is_meeting {
        "meeting_chunk"
    } else if output_pasted {
        "pasted"
    } else {
        "manual_paste"
    };
    let output_message = if is_meeting {
        "Sent to meeting".to_string()
    } else if output_pasted {
        "Pasted".to_string()
    } else {
        format!("Press {} to paste anywhere", paste_latest_hotkey_label())
    };
    // Divo turns never produce a paste — the Divo bridge owns the HUD from here.
    if !is_divo {
        tracing::debug!("[main] voice-output status={output_status}");
        let _ = app.emit(
            "voice-output",
            serde_json::json!({
                "status": output_status,
                "message": output_message,
                "run_id": client_run_id.as_deref(),
            }),
        );
    }

    if !is_divo {
        let mut paste_trace = said_core::dictation_trace::DictationTrace::default();
        paste_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "desktop.final_insert",
            component: "desktop",
            function: "insert_text_prefer_direct",
            output: Some(done.polished.as_str()),
            reason: Some("desktop inserted the final server output once"),
            risk: Some("paste_insert"),
            metadata: serde_json::json!({
                "output_pasted": output_pasted,
                "used_clipboard_fallback": used_clipboard_fallback,
                "output_status": output_status,
                "initial_field_readable": initial_field_text.as_ref().is_some_and(|s| !s.is_empty()),
                "message_polish": message_polish_mode,
                "suppress_local": suppress_local,
                "superseded": superseded,
            }),
            ..Default::default()
        });
        paste_trace.set_summary_field("paste_success", serde_json::json!(output_pasted));
        paste_trace.set_summary_field("output_status", serde_json::json!(output_status));
        let trace_ep = ep.clone();
        let trace_recording_id = done.recording_id.clone();
        let trace_value = paste_trace.into_value();
        tauri::async_runtime::spawn(async move {
            if let Err(e) =
                api::patch_dictation_trace(&trace_ep, &trace_recording_id, trace_value).await
            {
                tracing::warn!(
                    "[observability] dictation paste trace patch failed recording_id={trace_recording_id}: {e}"
                );
            }
        });
    }

    if let Some(run_id) = client_run_id.as_deref() {
        if let Err(e) = api::mark_voice_run_paste(&ep, run_id, output_pasted).await {
            tracing::warn!("[voice-run] paste status update failed run_id={run_id}: {e}");
        } else {
            tracing::info!(
                "[voice-run] paste status updated run_id={} paste_success={}",
                run_id,
                output_pasted,
            );
        }
        let mode = if is_divo {
            "divo"
        } else if is_meeting {
            "meeting"
        } else if message_polish_mode {
            "message_polish"
        } else {
            "normal_voice"
        };
        let audio_seconds = wav_len as f64 / (16_000.0 * 2.0);
        let word_count = done.polished.split_whitespace().count() as i32;
        let char_count = done.polished.chars().count() as i32;
        telemetry::on_pipeline_done(
            &ep,
            run_id,
            telemetry::PipelineTelemetry {
                recording_id: done.recording_id.clone(),
                mode: mode.to_string(),
                target_app: target_app_for_telemetry.clone(),
                audio_seconds,
                word_count,
                char_count,
                transcribe_ms: done.latency_ms.transcribe as i32,
                embed_ms: done.latency_ms.embed as i32,
                polish_ms: done.latency_ms.polish as i32,
                total_ms: done.latency_ms.total as i32,
                success: !done.polished.is_empty() || output_pasted,
                error_code: None,
                used_clipboard_fallback,
                speech_provider: speech_identity.provider.clone(),
                speech_model: speech_identity.model.clone(),
                speech_path: speech_identity.path.clone(),
                polished_preview: done.polished.clone(),
            },
        );
    }

    Ok(done)
}

async fn run_voice_repair_sse(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    action: &LastVoiceAction,
    app: &tauri::AppHandle,
) -> Result<api::PolishDone, String> {
    let ep = {
        let lock = back_arc.lock().map_err(|_| "backend lock failed")?;
        lock.clone().ok_or("backend not started")?
    };
    let transcript = action.raw_transcript.clone();
    let previous_output = action.polished.clone();
    let previous_output_for_replace = previous_output.clone();
    let target_app = action.target_app.clone();
    let output_language = action.output_language.clone();
    let audio_id = action.audio_id.clone();
    let enriched_transcript = action.enriched_transcript.clone();
    let app_clone = app.clone();

    let mut on_polish_event = move |event| match &event {
        api::PolishEvent::Token { token } => {
            let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
        }
        api::PolishEvent::Status { phase, transcript } => {
            emit_voice_status(&app_clone, phase, transcript.as_deref(), None);
        }
        api::PolishEvent::Done(done) => {
            emit_voice_done(&app_clone, done, None);
        }
        api::PolishEvent::Error {
            message,
            audio_id,
            error_code,
            ..
        } => {
            let human = humanize_error(message);
            let _ = app_clone.emit(
                "voice-error",
                serde_json::json!({
                    "message": human,
                    "audio_id": audio_id,
                    "error_code": error_code,
                }),
            );
        }
    };

    let done = api::stream_voice_repair(
        &ep,
        transcript,
        previous_output,
        target_app,
        output_language,
        audio_id,
        enriched_transcript,
        Some("user_requested_repair".into()),
        &mut on_polish_event,
    )
    .await?;

    finalize_repair_or_refine_output(
        app,
        &done,
        &previous_output_for_replace,
        LastRepairStage::FastRepair,
    )?;
    Ok(done)
}

async fn run_text_refine_sse(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    action: &LastTextTransformAction,
    app: &tauri::AppHandle,
) -> Result<api::PolishDone, String> {
    let ep = {
        let lock = back_arc.lock().map_err(|_| "backend lock failed")?;
        lock.clone().ok_or("backend not started")?
    };
    let source_text = action.source_text.clone();
    let previous_output = action.polished.clone();
    let previous_output_for_replace = previous_output.clone();
    let tone = Some(action.tone.clone());
    let app_clone = app.clone();

    let mut on_polish_event = move |event| match &event {
        api::PolishEvent::Token { token } => {
            let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
        }
        api::PolishEvent::Status { phase, transcript } => {
            emit_voice_status(&app_clone, phase, transcript.as_deref(), None);
        }
        api::PolishEvent::Done(done) => {
            emit_voice_done(&app_clone, done, None);
        }
        api::PolishEvent::Error {
            message,
            audio_id,
            error_code,
            ..
        } => {
            let _ = app_clone.emit(
                "voice-error",
                serde_json::json!({
                    "message": humanize_error(message),
                    "audio_id": audio_id,
                    "error_code": error_code,
                }),
            );
        }
    };

    let done = api::stream_text_refine_last(
        &ep,
        source_text.clone(),
        previous_output,
        tone.clone(),
        &mut on_polish_event,
    )
    .await?;

    finalize_repair_or_refine_output(
        app,
        &done,
        &previous_output_for_replace,
        LastRepairStage::None,
    )?;
    cache_last_text_transform(
        app,
        source_text,
        done.polished.clone(),
        tone.unwrap_or_else(|| "neutral".into()),
    );
    Ok(done)
}

fn finalize_repair_or_refine_output(
    app: &tauri::AppHandle,
    done: &api::PolishDone,
    previous_output: &str,
    repair_stage: LastRepairStage,
) -> Result<(), String> {
    let mut output_pasted = false;
    if !done.polished.is_empty() {
        match paster::replace_focused_text_exact(previous_output, &done.polished) {
            Ok(true) => {
                tracing::info!("[repair] replaced exact previous output in focused field");
                output_pasted = true;
            }
            Ok(false) => {
                tracing::warn!(
                    "[repair] exact previous output not found in focused field — storing latest only"
                );
            }
            Err(e) => {
                tracing::warn!("[repair] focused-field replacement failed: {e}");
            }
        }
    }

    if !done.polished.is_empty() {
        if let Ok(mut g) = app.state::<LatestResult>().0.lock() {
            *g = Some(done.polished.clone());
        }
        if matches!(done.source.as_deref(), Some("voice") | Some("voice_repair")) {
            cache_last_voice_action(app, done, repair_stage);
        }
    }

    let output_status = if output_pasted {
        "pasted"
    } else {
        "manual_paste"
    };
    let output_message = if output_pasted {
        "Repaired".to_string()
    } else {
        format!(
            "Repair ready — press {} to paste",
            paste_latest_hotkey_label()
        )
    };
    let _ = app.emit(
        "voice-output",
        serde_json::json!({
            "status": output_status,
            "message": output_message,
        }),
    );
    Ok(())
}

/// Paste the most-recently stored polished result into the focused app.
/// Invoked by the global paste-latest hotkey and by the UI's "Paste latest" button.
#[tauri::command]
fn paste_latest(latest: State<'_, LatestResult>) -> Result<bool, String> {
    let text = {
        let g = latest.0.lock().map_err(|_| "lock failed")?;
        g.clone()
    };
    match text {
        None => {
            tracing::info!("[paste_latest] nothing stored yet");
            Ok(false)
        }
        Some(t) => {
            tracing::info!("[paste_latest] pasting {} chars", t.len());
            // Direct-first (never clobbers the clipboard); clipboard only if
            // injection is blocked.
            paster::insert_text(&t).map_err(|e| format!("paste failed: {e}"))?;
            Ok(true)
        }
    }
}

fn run_fast_voice_repair(app: tauri::AppHandle, action: LastVoiceAction) {
    let shared = app.state::<SharedApp>();
    let backend = app.state::<BackendState>();
    {
        let Ok(mut d) = shared.0.lock() else { return };
        if d.state != desktop::AppState::Idle {
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": "Busy — wait for the current operation to finish.",
                    "audio_id": null,
                }),
            );
            return;
        }
        d.state = desktop::AppState::Processing;
    }

    let shared2 = Arc::clone(&shared.0);
    let app2 = app.clone();
    let back_arc2 = Arc::clone(&backend.0);
    tauri::async_runtime::spawn(async move {
        let result = run_voice_repair_sse(&back_arc2, &action, &app2).await;
        let snap = {
            let mut d = match shared2.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match result {
                Ok(done) => d.finish_ok(ProcessSummary {
                    transcript: done.transcript.clone(),
                    polished: done.polished,
                    model: done.model_used,
                    confidence: done.confidence.unwrap_or(0.0),
                    transcribe_ms: done.latency_ms.transcribe as u64,
                    polish_ms: done.latency_ms.polish as u64,
                }),
                Err(e) => d.finish_err(strip_voice_error_already_emitted(&e).to_string()),
            }
        };
        sync_tray(&app2, &snap);
        emit_app_state(&app2, &snap);
    });
}

fn run_refine_last_transform(app: tauri::AppHandle, action: LastTextTransformAction) {
    let shared = app.state::<SharedApp>();
    let backend = app.state::<BackendState>();
    {
        let Ok(mut d) = shared.0.lock() else { return };
        if d.state != desktop::AppState::Idle {
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": "Busy — wait for the current operation to finish.",
                    "audio_id": null,
                }),
            );
            return;
        }
        d.state = desktop::AppState::Processing;
    }

    let shared2 = Arc::clone(&shared.0);
    let app2 = app.clone();
    let back_arc2 = Arc::clone(&backend.0);
    tauri::async_runtime::spawn(async move {
        let result = run_text_refine_sse(&back_arc2, &action, &app2).await;
        let snap = {
            let mut d = match shared2.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match result {
                Ok(done) => d.finish_ok(ProcessSummary {
                    transcript: done.transcript.clone(),
                    polished: done.polished,
                    model: done.model_used,
                    confidence: done.confidence.unwrap_or(0.0),
                    transcribe_ms: done.latency_ms.transcribe as u64,
                    polish_ms: done.latency_ms.polish as u64,
                }),
                Err(e) => d.finish_err(e),
            }
        };
        sync_tray(&app2, &snap);
        emit_app_state(&app2, &snap);
    });
}

/// Delete a recording from the backend (SQLite + WAV file).
#[tauri::command]
async fn delete_recording(backend: State<'_, BackendState>, id: String) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::delete_recording(&ep, &id).await
}

/// Return the bearer-authed URL to stream a recording's WAV audio.
/// The frontend fetches this URL with the Authorization header to get a blob.
#[tauri::command]
fn get_recording_audio_url(
    backend: State<'_, BackendState>,
    id: String,
) -> Result<serde_json::Value, String> {
    let ep = get_endpoint(&backend)?;
    let url = api::recording_audio_url(&ep, &id);
    let secret = ep.secret.clone();
    Ok(serde_json::json!({ "url": url, "secret": secret }))
}

/// Return WAV bytes for in-app playback without requiring the webview to make
/// an authenticated localhost fetch.
#[tauri::command]
async fn get_recording_audio_bytes(
    backend: State<'_, BackendState>,
    id: String,
) -> Result<Vec<u8>, String> {
    let ep = get_endpoint(&backend)?;
    api::recording_audio_bytes(&ep, &id).await
}

fn safe_download_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "said-recording.wav".to_string()
    } else if trimmed.to_lowercase().ends_with(".wav") {
        trimmed
    } else {
        format!("{trimmed}.wav")
    }
}

fn unique_download_path(dir: &std::path::Path, filename: &str) -> std::path::PathBuf {
    let initial = dir.join(filename);
    if !initial.exists() {
        return initial;
    }

    let path = std::path::Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("said-recording");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("wav");
    for n in 2..1000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("{stem}-copy.{ext}"))
}

fn applescript_string(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', " ");
    format!("\"{escaped}\"")
}

fn choose_recording_audio_save_path(filename: &str) -> Result<Option<std::path::PathBuf>, String> {
    let filename = safe_download_filename(filename);

    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "set chosenFile to choose file name with prompt {} default name {} default location (path to downloads folder)\nPOSIX path of chosenFile",
            applescript_string("Save AirNote audio recording"),
            applescript_string(&filename),
        );
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| format!("save dialog failed: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if stderr.contains("-128") || stderr.to_lowercase().contains("user canceled") {
                return Ok(None);
            }
            return Err(format!("save dialog failed: {}", stderr.trim()));
        }

        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if path.is_empty() {
            Ok(None)
        } else {
            Ok(Some(std::path::PathBuf::from(path)))
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .ok_or_else(|| "Downloads folder not found".to_string())?;
        Ok(Some(unique_download_path(&dir, &filename)))
    }
}

/// Save a recording WAV via native filesystem IO. This avoids WKWebView
/// blob-anchor download behavior, which is unreliable in packaged desktop apps.
#[tauri::command]
async fn download_recording_audio(
    backend: State<'_, BackendState>,
    id: String,
    filename: String,
) -> Result<Option<String>, String> {
    let Some(path) = choose_recording_audio_save_path(&filename)? else {
        return Ok(None);
    };
    let ep = get_endpoint(&backend)?;
    let bytes = api::recording_audio_bytes(&ep, &id).await?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create download folder: {e}"))?;
    }
    std::fs::write(&path, bytes).map_err(|e| format!("couldn't save audio: {e}"))?;

    Ok(Some(path.display().to_string()))
}

/// Save a meeting's local audio file to a user-chosen location (native save
/// dialog on macOS, Downloads elsewhere). The file is already on disk, so this
/// copies it rather than fetching from the server. Returns the saved path, or
/// None if the user cancelled.
#[tauri::command]
fn download_meeting_audio(audio_path: String, filename: String) -> Result<Option<String>, String> {
    let src = std::path::PathBuf::from(&audio_path);
    if !src.is_file() {
        return Err("meeting audio file not found on disk".to_string());
    }
    let Some(dest) = choose_recording_audio_save_path(&filename)? else {
        return Ok(None);
    };
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create download folder: {e}"))?;
    }
    std::fs::copy(&src, &dest).map_err(|e| format!("couldn't save audio: {e}"))?;
    Ok(Some(dest.display().to_string()))
}

/// Export the History transcripts to a user-chosen text/markdown file. The
/// caller builds the full document (Markdown/plain text) in the webview; this
/// just shows the native save dialog and writes it. Returns the saved path, or
/// None if the user cancelled.
#[tauri::command]
fn export_history(content: String, filename: String) -> Result<Option<String>, String> {
    let Some(path) = choose_recording_audio_save_path(&filename)? else {
        return Ok(None);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("couldn't create export folder: {e}"))?;
    }
    std::fs::write(&path, content).map_err(|e| format!("couldn't save export: {e}"))?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
fn reveal_downloaded_file(path: String) -> Result<(), String> {
    let path = std::path::PathBuf::from(path);
    if !path.exists() {
        return Err("downloaded file no longer exists".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .status()
            .map_err(|e| format!("couldn't reveal file: {e}"))?;
        return Ok(());
    }

    #[cfg(not(target_os = "macos"))]
    {
        let target = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        open::that(target).map_err(|e| format!("couldn't open containing folder: {e}"))?;
        Ok(())
    }
}

#[tauri::command]
fn reveal_saved_audio(audio_id: String) -> Result<(), String> {
    let path = saved_voice_audio_path(&audio_id)?;
    if !path.is_file() {
        return Err("saved audio file no longer exists".to_string());
    }
    tracing::info!(
        "[retry] revealing saved audio audio_id={} path={}",
        audio_id,
        path.display(),
    );
    reveal_downloaded_file(path.display().to_string())
}

fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("pbcopy failed to start: {e}"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("pbcopy write failed: {e}"))?;
        }
        let status = child
            .wait()
            .map_err(|e| format!("pbcopy wait failed: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("pbcopy exited with {status}"));
    }

    #[cfg(target_os = "windows")]
    {
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("clip failed to start: {e}"))?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("clip write failed: {e}"))?;
        }
        let status = child.wait().map_err(|e| format!("clip wait failed: {e}"))?;
        if status.success() {
            return Ok(());
        }
        return Err(format!("clip exited with {status}"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = text;
        Err("clipboard copy is not supported on this platform".to_string())
    }
}

/// Retry a failed recording by re-submitting its saved WAV file.
/// `audio_id` is the UUID that the backend included in the `voice-error` event.
#[tauri::command]
fn retry_recording(
    audio_id: String,
    state: State<'_, SharedApp>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let message_polish_mode = failed_voice_mode_for_audio(&app, &audio_id);
    retry_recording_spawn(
        audio_id,
        Arc::clone(&state.0),
        Arc::clone(&backend.0),
        app,
        message_polish_mode,
        "explicit",
    )
}

fn retry_recording_internal(app: tauri::AppHandle, audio_id: String) {
    retry_recording_internal_with_mode(app, audio_id, None, "internal");
}

fn retry_recording_internal_with_mode(
    app: tauri::AppHandle,
    audio_id: String,
    message_polish_mode: Option<bool>,
    source: &'static str,
) {
    let shared = Arc::clone(&app.state::<SharedApp>().0);
    let backend = Arc::clone(&app.state::<BackendState>().0);
    let _ = retry_recording_spawn(audio_id, shared, backend, app, message_polish_mode, source);
}

fn retry_recording_spawn(
    audio_id: String,
    shared: Arc<Mutex<DesktopApp>>,
    backend: Arc<Mutex<Option<BackendEndpoint>>>,
    app: tauri::AppHandle,
    message_polish_mode: Option<bool>,
    source: &'static str,
) -> Result<(), String> {
    // Read WAV from the saved file
    let wav_path = saved_voice_audio_path(&audio_id)?;
    let wav = std::fs::read(&wav_path).map_err(|e| format!("saved audio not found: {e}"))?;
    tracing::info!(
        "[retry] starting saved-audio retry source={} audio_id={} wav_bytes={} message_polish_override={:?} path={}",
        source,
        audio_id,
        wav.len(),
        message_polish_mode,
        wav_path.display(),
    );

    // Mark as processing so the UI shows a spinner
    {
        let mut d = shared.lock().map_err(|_| "lock failed")?;
        if d.state != desktop::AppState::Idle {
            return Err("busy — wait for current operation to finish".into());
        }
        d.state = desktop::AppState::Processing;
    }

    let shared2 = Arc::clone(&shared);
    let app2 = app.clone();
    let back_arc2 = Arc::clone(&backend);

    tauri::async_runtime::spawn(async move {
        let stt_language = app2
            .try_state::<HotPathCache>()
            .and_then(|hot| hot.0.try_read().ok().map(|guard| guard.language.clone()))
            .filter(|lang| !lang.is_empty())
            .unwrap_or_else(|| "hi".to_string());
        // The polish backend requires a pre_transcript, so an STT failure ends
        // the retry here with the provider's own actionable message instead of
        // a confusing backend 400.
        let pre_transcript = match stt_pre_transcript(&wav, &stt_language).await {
            Ok(t) => Some(t),
            Err(message) => {
                tracing::warn!("[retry] speech transcription failed: {message}");
                emit_voice_error_quiet(&app2, &message);
                let snap = {
                    let mut d = match shared2.lock() {
                        Ok(g) => g,
                        Err(_) => return,
                    };
                    d.finish_err(message)
                };
                sync_tray(&app2, &snap);
                emit_app_state(&app2, &snap);
                return;
            }
        };
        let result = run_voice_polish_sse(
            &back_arc2,
            wav,
            None,
            None,
            pre_transcript,
            Some("preserve_recall".into()),
            None, // no screen context for re-polish
            message_polish_mode,
            &app2,
            false,
            false, // not a Divo turn
        )
        .await;

        let watch_start = std::time::Instant::now();
        if let Ok(ref done) = result {
            let back3 = Arc::clone(&back_arc2);
            let anchor = capture_edit_anchor(None, None, &done.polished);
            start_edit_watcher(
                back3,
                app2.clone(),
                None,
                done.recording_id.clone(),
                done.polished.clone(),
                watch_start,
                anchor,
            );
        }

        let snap = {
            let mut d = match shared2.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            match result {
                Ok(done) => d.finish_ok(ProcessSummary {
                    transcript: done.transcript.clone(),
                    polished: done.polished,
                    model: done.model_used,
                    confidence: done.confidence.unwrap_or(0.0),
                    transcribe_ms: done.latency_ms.transcribe as u64,
                    polish_ms: done.latency_ms.polish as u64,
                }),
                Err(e) => d.finish_err(e),
            }
        };
        sync_tray(&app2, &snap);
        emit_app_state(&app2, &snap);
    });

    Ok(())
}

// ── Pending-edit review commands ──────────────────────────────────────────────

#[tauri::command]
async fn get_pending_edits(
    backend: State<'_, BackendState>,
) -> Result<api::PendingEditsResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::get_pending_edits(&ep).await
}

#[tauri::command]
async fn resolve_pending_edit(
    backend: State<'_, BackendState>,
    id: String,
    action: String,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let result = api::resolve_pending_edit(&ep, &id, &action).await;
    if result.is_ok() && action == "approve" {
        tracing::info!("[vocabulary] pending edit approved; local ASR has no runtime bias refresh");
    }
    result
}

#[tauri::command]
async fn dismiss_pending_edit(backend: State<'_, BackendState>, id: String) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::dismiss_pending_edit(&ep, &id).await
}

// ── Vocabulary management commands ────────────────────────────────────────────

#[tauri::command]
async fn list_vocabulary(
    backend: State<'_, BackendState>,
) -> Result<api::VocabListResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::list_vocabulary(&ep).await
}

#[tauri::command]
async fn list_vocabulary_aliases(
    backend: State<'_, BackendState>,
) -> Result<api::AliasesResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::list_vocab_aliases(&ep).await
}

#[tauri::command]
async fn add_vocabulary_term(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    term: String,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::add_vocabulary_term(&ep, &term).await?;
    let _ = app.emit("vocabulary-changed", ());

    // In-app toast (matches the website's design language) — primary surface.
    let _ = app.emit(
        "vocab-toast",
        serde_json::json!({
            "kind": "added", "term": term, "source": "manual",
        }),
    );

    // OS-level fallback for when the AirNote window isn't focused.
    notify_macos(
        &app,
        "Added to vocabulary",
        &format!("AirNote will recognise \"{term}\" on your next recording."),
    );
    Ok(())
}

#[tauri::command]
async fn delete_vocabulary_term(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    term: String,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::delete_vocabulary_term(&ep, &term).await?;
    let _ = app.emit("vocabulary-changed", ());
    let _ = app.emit(
        "vocab-toast",
        serde_json::json!({
            "kind": "removed", "term": term,
        }),
    );
    Ok(())
}

#[tauri::command]
async fn confirm_term(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    term: String,
    original: String,
    action: String,
    recording_id: Option<String>,
    context: Option<String>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let body = serde_json::json!({
        "term": term,
        "original": original,
        "action": action,
        "recording_id": recording_id,
        "context": context,
    });
    let url = format!("{}/v1/confirm-term", ep.url);
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("confirm-term failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("confirm-term returned {}", resp.status()));
    }
    if action == "learn" {
        let _ = app.emit("vocabulary-changed", ());
        tracing::info!("[confirm] user confirmed term {:?} — learning", term);
    } else {
        tracing::info!("[confirm] user skipped term {:?}", term);
    }
    Ok(())
}

#[tauri::command]
async fn confirm_batch(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    items: Vec<serde_json::Value>,
    recording_id: Option<String>,
    review_session_id: Option<String>,
) -> Result<api::ConfirmBatchResponse, String> {
    let ep = get_endpoint(&backend)?;
    let request_items: Vec<api::ConfirmBatchRequestItem> = items
        .iter()
        .filter_map(|v| {
            let orig = v.get("original")?.as_str()?.to_string();
            let corr = v.get("corrected")?.as_str()?.to_string();
            let context = v
                .get("context")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());
            let tag = v
                .get("tag")
                .and_then(|value| value.as_str())
                .map(|value| value.to_string());
            Some(api::ConfirmBatchRequestItem {
                original: orig,
                corrected: corr,
                context,
                tag,
            })
        })
        .collect();
    let result = api::confirm_batch(
        &ep,
        &request_items,
        recording_id.as_deref(),
        review_session_id.as_deref(),
    )
    .await?;
    let _ = app.emit("vocabulary-changed", ());
    tracing::info!(
        "[confirm-batch] user confirmed {} term(s) server_owned={}: {:?}",
        result.learned_count,
        result.server_owned,
        result.learned_terms,
    );
    Ok(result)
}

#[tauri::command]
async fn get_next_edit_review_session(
    backend: State<'_, BackendState>,
) -> Result<Option<api::EditReviewSessionResponse>, String> {
    let ep = get_endpoint(&backend)?;
    api::get_next_edit_review_session(&ep).await
}

#[tauri::command]
async fn skip_edit_review_session(
    backend: State<'_, BackendState>,
    session_id: String,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::skip_edit_review_session(&ep, &session_id).await
}

#[tauri::command]
async fn block_correction(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    variant: String,
    wrong_replacement: String,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let body = serde_json::json!({
        "variant": variant,
        "wrong_replacement": wrong_replacement,
    });
    let url = format!("{}/v1/block-correction", ep.url);
    let resp = reqwest::Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("block-correction failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("block-correction returned {}", resp.status()));
    }
    let _ = app.emit("vocabulary-changed", ());
    tracing::info!(
        "[block] user blocked correction {:?} → {:?}",
        variant,
        wrong_replacement
    );
    Ok(())
}

#[tauri::command]
async fn reset_all_vocabulary(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::reset_all_vocabulary(&ep).await?;
    let _ = app.emit("vocabulary-changed", ());
    Ok(())
}

#[tauri::command]
async fn star_vocabulary_term(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    term: String,
) -> Result<bool, String> {
    let ep = get_endpoint(&backend)?;
    let starred = api::star_vocabulary_term(&ep, &term).await?;
    let _ = app.emit("vocabulary-changed", ());

    // Lightweight confirmation toast for star/unstar — only on STAR (positive
    // affirmation), not on unstar (silent).
    if starred {
        let _ = app.emit(
            "vocab-toast",
            serde_json::json!({
                "kind": "starred", "term": term,
            }),
        );
        notify_macos(
            &app,
            "Pinned to vocabulary",
            &format!("AirNote will keep \"{term}\" even if you stop using it."),
        );
    }
    Ok(starred)
}

#[tauri::command]
async fn patch_vocabulary_term(
    app: tauri::AppHandle,
    backend: State<'_, BackendState>,
    term: String,
    meaning: Option<String>,
    term_type: Option<String>,
    example_context: Option<String>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::patch_vocabulary_term(
        &ep,
        &term,
        meaning.as_deref(),
        term_type.as_deref(),
        example_context.as_deref(),
    )
    .await?;
    let _ = app.emit("vocabulary-changed", ());
    Ok(())
}

// ── Invite-a-friend ───────────────────────────────────────────────────────────

/// Outcome of an invite send attempt — lets the frontend either celebrate
/// the server-side send or seamlessly fall back to opening the user's mail app.
#[derive(serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum InviteOutcome {
    /// Email was sent server-side via Resend.
    Sent,
    /// Server has no email provider configured (RESEND_API_KEY missing).
    /// Frontend should open `mailto:` so the user can still send via their
    /// own mail client.
    FallbackMailto,
}

#[tauri::command]
async fn send_invite_email(
    backend: State<'_, BackendState>,
    to: String,
) -> Result<InviteOutcome, String> {
    let ep = get_endpoint(&backend)?;
    match api::send_invite_email(&ep, &to).await {
        Ok(_) => Ok(InviteOutcome::Sent),
        Err(e) if e == "email_not_configured" => Ok(InviteOutcome::FallbackMailto),
        Err(e) => Err(e),
    }
}

// ── External URL opener ───────────────────────────────────────────────────────

/// Open a URL (https://, mailto:, etc.) in the user's default app.
///
/// Tauri's webview blocks `window.open("mailto:…")` silently — calls fall
/// through to the browser's noop handler, so the user sees nothing happen.
/// This command shells out to the OS opener instead.

// ── OpenAI / ChatGPT OAuth ───────────────────────────────────────────────────

#[derive(serde::Serialize)]
struct OpenAIStatus {
    connected: bool,
    expires_at: Option<i64>,
    connected_at: Option<i64>,
}

#[tauri::command]
async fn openai_connect(app: tauri::AppHandle) -> Result<String, String> {
    let ep = {
        let st = app.state::<BackendState>();
        st.0.lock().ok().and_then(|g| g.clone())
    }
    .ok_or("backend not ready")?;

    let url = format!("{}/v1/openai-oauth/initiate", ep.url);
    let resp: serde_json::Value = reqwest::Client::new()
        .post(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("initiate failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    let auth_url = resp
        .get("auth_url")
        .and_then(|u| u.as_str())
        .ok_or("no auth_url in response")?
        .to_string();

    open_external(auth_url.clone())?;
    Ok(auth_url)
}

#[tauri::command]
async fn openai_status(app: tauri::AppHandle) -> Result<OpenAIStatus, String> {
    let ep = {
        let st = app.state::<BackendState>();
        st.0.lock().ok().and_then(|g| g.clone())
    }
    .ok_or("backend not ready")?;

    let url = format!("{}/v1/openai-oauth/status", ep.url);
    let resp: serde_json::Value = reqwest::Client::new()
        .get(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("status failed: {e}"))?
        .json()
        .await
        .map_err(|e| format!("parse failed: {e}"))?;

    Ok(OpenAIStatus {
        connected: resp
            .get("connected")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        expires_at: resp.get("expires_at").and_then(|v| v.as_i64()),
        connected_at: resp.get("connected_at").and_then(|v| v.as_i64()),
    })
}

#[tauri::command]
async fn openai_disconnect(app: tauri::AppHandle) -> Result<(), String> {
    let ep = {
        let st = app.state::<BackendState>();
        st.0.lock().ok().and_then(|g| g.clone())
    }
    .ok_or("backend not ready")?;

    let url = format!("{}/v1/openai-oauth/disconnect", ep.url);
    reqwest::Client::new()
        .delete(&url)
        .header("Authorization", ep.bearer())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("disconnect failed: {e}"))?;
    Ok(())
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    use std::process::Command;

    // Defence in depth: only allow safe schemes. We never pass arbitrary
    // shell to `open` (it's argv-based) but reject schemes that don't make
    // sense for a "click a link" handler.
    let lower = url.to_ascii_lowercase();
    let ok = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:");
    if !ok {
        return Err(format!("refusing to open scheme: {url}"));
    }

    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(&url).spawn();

    #[cfg(target_os = "windows")]
    let result = Command::new("cmd").args(["/C", "start", "", &url]).spawn();

    #[cfg(all(unix, not(target_os = "macos")))]
    let result = Command::new("xdg-open").arg(&url).spawn();

    result
        .map(|_| ())
        .map_err(|e| format!("failed to open: {e}"))
}

// ── Desktop-only prefs (Sentry on/off + update channel) ──────────────────────
//
// These live in `<data_dir>/desktop_prefs.json` rather than the backend's
// SQLite preferences DB because they must be readable synchronously at
// process startup — before the backend daemon is reachable. See
// `said_core::prefs` for details.
//
// Changes take effect on next launch (Sentry init and the updater plugin
// each read once during Tauri setup). The Settings UI shows a "restart to
// apply" hint next to both toggles.

#[tauri::command]
fn get_desktop_prefs() -> said_core::prefs::DesktopPrefs {
    stt_policy::normalize_persisted_prefs().unwrap_or_else(|error| {
        tracing::warn!("[stt-policy] couldn't persist normalized preferences: {error}");
        stt_policy::normalize_prefs(said_core::prefs::load())
    })
}

#[tauri::command]
fn set_desktop_prefs(
    app: tauri::AppHandle,
    prefs: said_core::prefs::DesktopPrefs,
) -> Result<(), String> {
    let previous = said_core::prefs::load();
    let prefs = stt_policy::normalize_prefs(prefs);
    said_core::prefs::save(&prefs)?;
    if previous.launch_at_login != prefs.launch_at_login {
        apply_launch_at_login(&app, prefs.launch_at_login)?;
    }
    if previous.dictation_stt != prefs.dictation_stt
        || previous.local_stt_model != prefs.local_stt_model
    {
        if nemotron::is_nemotron_pref(&previous.local_stt_model)
            && !nemotron::is_nemotron_pref(&prefs.local_stt_model)
        {
            // The cache owns roughly the model's mapped memory. Releasing it
            // on switch keeps the experimental option from consuming RAM
            // after the user has returned to the stable Whisper path.
            nemotron::unload();
        }
        // Warm the newly selected provider off-thread so the next dictation
        // doesn't pay its setup cost (model load / HTTP client).
        tracing::info!(
            from = %previous.dictation_stt,
            to = %prefs.dictation_stt,
            previous_local_model = %previous.local_stt_model,
            local_model = %prefs.local_stt_model,
            "[prefs] dictation STT provider changed — prewarming"
        );
        std::thread::Builder::new()
            .name("dictation-stt-prewarm".into())
            .spawn(dictation_stt::prewarm)
            .ok();
    }
    Ok(())
}

// ── Developer log viewer (Settings → Developer log) ──────────────────────────
//
// Surfaces the backend daemon's `backend.log` in-app so issues (e.g. a failed
// vocabulary write) can be diagnosed without hunting through the OS data dir.

fn backend_log_path() -> std::path::PathBuf {
    said_core::paths::log_dir().join("backend.log")
}

/// Return the tail of the backend log (last `max_lines` lines, default 600).
#[tauri::command]
fn read_backend_log(max_lines: Option<usize>) -> Result<String, String> {
    let path = backend_log_path();
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let max = max_lines.unwrap_or(600);
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max);
    Ok(lines[start..].join("\n"))
}

/// Absolute path of the backend log file (shown in the UI).
#[tauri::command]
fn backend_log_location() -> String {
    backend_log_path().to_string_lossy().into_owned()
}

/// Reveal the log directory in the OS file manager.
#[tauri::command]
fn open_log_folder() -> Result<(), String> {
    let dir = said_core::paths::log_dir();
    open::that(&dir).map_err(|e| format!("couldn't open {}: {e}", dir.display()))
}

// ── Meeting mode commands ────────────────────────────────────────────────────

/// Enter meeting mode: auto-start recording, invert hotkey (hold = mute).
/// Polished text is emitted as `meeting-transcript` events instead of typed.
#[tauri::command]
fn start_meeting_stt(
    app: tauri::AppHandle,
    meeting_mode: State<'_, MeetingModeState>,
    state: State<'_, SharedApp>,
) -> Result<MeetingSttStatus, String> {
    let was_inactive = meeting_mode.enter();
    tracing::info!("[meeting_mode] entered — auto-starting recording");
    if let Err(err) = meeting_mode.ensure_echo_reference() {
        tracing::warn!("[meeting_mode] speaker filter unavailable: {err}");
        meeting_mode
            .echo_gate
            .mark_reference_unavailable(err.clone());
        let _ = app.emit(
            "voice-error",
            serde_json::json!({
                "message": "Speaker filter unavailable",
                "raw_error": err,
                "audio_id": null,
                "auto_hide_ms": 3500,
            }),
        );
    }
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    if was_inactive || current == desktop::AppState::Idle {
        do_start_recording(&state.0, &app);
    }
    emit_meeting_stt_status(&app);
    Ok(meeting_mode.status())
}

/// Leave meeting mode: stop recording if active, restore normal hotkey behavior.
#[tauri::command]
fn stop_meeting_stt(
    app: tauri::AppHandle,
    meeting_mode: State<'_, MeetingModeState>,
    state: State<'_, SharedApp>,
    _backend: State<'_, BackendState>,
) -> Result<MeetingSttStatus, String> {
    let was = meeting_mode.exit();
    if !was {
        emit_meeting_stt_status(&app);
        return Ok(meeting_mode.status()); // already not in meeting mode
    }
    tracing::info!("[meeting_mode] exited — stopping recording");
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    let route = app
        .state::<RecordingRouteState>()
        .0
        .lock()
        .ok()
        .and_then(|route| *route);
    if current == desktop::AppState::Recording && route == Some(RecordingRoute::Meeting) {
        do_cancel_recording(Arc::clone(&state.0), app, "leave meeting");
    } else {
        emit_meeting_stt_status(&app);
    }
    Ok(meeting_mode.status())
}

/// Toggle meeting capture. Muted meeting mode leaves normal AirNote dictation available.
#[tauri::command]
fn toggle_meeting_mute(
    app: tauri::AppHandle,
    meeting_mode: State<'_, MeetingModeState>,
    state: State<'_, SharedApp>,
    _backend: State<'_, BackendState>,
) -> Result<MeetingSttStatus, String> {
    if !meeting_mode.is_active() {
        return Err("not in meeting mode".into());
    }
    let current = state.0.lock().map_err(|_| "lock failed")?.state;
    let route = app
        .state::<RecordingRouteState>()
        .0
        .lock()
        .ok()
        .and_then(|route| *route);

    if meeting_mode.is_muted() {
        if current != desktop::AppState::Idle {
            return Err(
                "finish the current AirNote recording before resuming meeting capture".into(),
            );
        }
        meeting_mode.set_muted(false);
        if let Err(err) = meeting_mode.ensure_echo_reference() {
            tracing::warn!("[meeting_mode] speaker filter unavailable after resume: {err}");
            meeting_mode
                .echo_gate
                .mark_reference_unavailable(err.clone());
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": "Speaker filter unavailable",
                    "raw_error": err,
                    "audio_id": null,
                    "auto_hide_ms": 3500,
                }),
            );
        }
        emit_meeting_stt_status(&app);
        do_start_recording(&state.0, &app);
        return Ok(meeting_mode.status());
    }

    meeting_mode.set_muted(true);
    emit_meeting_stt_status(&app);
    if current == desktop::AppState::Recording && route == Some(RecordingRoute::Meeting) {
        do_cancel_recording(Arc::clone(&state.0), app, "mute");
    }
    Ok(meeting_mode.status())
}

#[tauri::command]
fn get_meeting_stt_status(
    app: tauri::AppHandle,
    meeting_mode: State<'_, MeetingModeState>,
) -> MeetingSttStatus {
    let status = meeting_mode.status();
    let _ = app.emit("meeting-stt-state", status.clone());
    status
}

// ── Cloud auth commands ───────────────────────────────────────────────────────

/// Cloud URL — read from env, default to the hosted service.
fn cloud_url() -> String {
    std::env::var("CLOUD_API_URL").unwrap_or_else(|_| "https://cloud.voicepolish.app".into())
}

#[tauri::command]
async fn cloud_signup(
    email: String,
    password: String,
    backend: State<'_, BackendState>,
) -> Result<api::CloudAuthResponse, String> {
    let resp = api::cloud_signup(&cloud_url(), &email, &password).await?;
    // Persist token in local backend SQLite
    if let Ok(ep) = get_endpoint(&backend) {
        let _ = api::store_cloud_token(&ep, &resp.token, &resp.account.license_tier).await;
    }
    Ok(resp)
}

#[tauri::command]
async fn cloud_login(
    email: String,
    password: String,
    backend: State<'_, BackendState>,
) -> Result<api::CloudAuthResponse, String> {
    let resp = api::cloud_login(&cloud_url(), &email, &password).await?;
    if let Ok(ep) = get_endpoint(&backend) {
        let _ = api::store_cloud_token(&ep, &resp.token, &resp.account.license_tier).await;
    }
    Ok(resp)
}

#[tauri::command]
async fn start_enterprise_oauth_listener(app: tauri::AppHandle) -> Result<u16, String> {
    enterprise_oauth::start_listener(app).await
}

#[tauri::command]
fn stop_enterprise_oauth_listener() {
    enterprise_oauth::stop_listener();
}

#[tauri::command]
async fn store_enterprise_auth(
    token: String,
    email: String,
    server_url: String,
    org_name: Option<String>,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::store_enterprise_token(
        &ep,
        &token,
        "enterprise",
        &email,
        &server_url,
        org_name.as_deref(),
    )
    .await
}

#[tauri::command]
async fn clear_enterprise_auth(backend: State<'_, BackendState>) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::clear_cloud_token(&ep).await
}

#[tauri::command]
async fn get_enterprise_status(
    backend: State<'_, BackendState>,
) -> Result<api::EnterpriseStatus, String> {
    let ep = get_endpoint(&backend)?;
    api::get_enterprise_status(&ep).await
}

#[tauri::command]
async fn list_workspaces(
    backend: State<'_, BackendState>,
) -> Result<api::WorkspaceListResponse, String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_enterprise_status(&ep).await?;
    let token = status
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "not signed in to a workspace".to_string())?;
    let server_url = status
        .server_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "workspace server URL not configured".to_string())?;
    api::list_workspaces(&server_url, &token, status.active_org_id.as_deref()).await
}

/// What the cloud profiling brain has learned (run stats + KB + per-bucket style).
#[tauri::command]
async fn get_profile_insights(
    backend: State<'_, BackendState>,
) -> Result<api::ProfileInsights, String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_enterprise_status(&ep).await?;
    let token = status
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "not signed in to a workspace".to_string())?;
    let server_url = status
        .server_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "workspace server URL not configured".to_string())?;
    api::get_profile_insights(&server_url, &token, status.active_org_id.as_deref()).await
}

/// Apps the user dictates into, grouped by bucket (for the Buckets kanban).
#[tauri::command]
async fn get_app_buckets(backend: State<'_, BackendState>) -> Result<serde_json::Value, String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_enterprise_status(&ep).await?;
    let token = status
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "not signed in to a workspace".to_string())?;
    let server_url = status
        .server_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "workspace server URL not configured".to_string())?;
    api::get_app_buckets(&server_url, &token, status.active_org_id.as_deref()).await
}

/// Re-file an app into a bucket (user override; wins over static + agent mappings).
#[tauri::command]
async fn set_app_bucket(
    app_key: String,
    bucket_key: String,
    backend: State<'_, BackendState>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_enterprise_status(&ep).await?;
    let token = status
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "not signed in to a workspace".to_string())?;
    let server_url = status
        .server_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "workspace server URL not configured".to_string())?;
    api::set_app_bucket(
        &server_url,
        &token,
        status.active_org_id.as_deref(),
        &app_key,
        &bucket_key,
    )
    .await
}

#[tauri::command]
async fn activate_workspace(
    org_id: String,
    backend: State<'_, BackendState>,
) -> Result<String, String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_enterprise_status(&ep).await?;
    let token = status
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "not signed in to a workspace".to_string())?;
    let server_url = status
        .server_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "workspace server URL not configured".to_string())?;
    let active = api::activate_workspace_on_server(&server_url, &token, &org_id).await?;
    api::set_local_active_org(&ep, Some(&active)).await?;
    Ok(active)
}

#[tauri::command]
async fn deactivate_workspace(backend: State<'_, BackendState>) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_enterprise_status(&ep).await?;
    let token = status
        .token
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "not signed in to a workspace".to_string())?;
    let server_url = status
        .server_url
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "workspace server URL not configured".to_string())?;
    api::deactivate_workspace_on_server(&server_url, &token).await?;
    api::set_local_active_org(&ep, None).await
}

#[tauri::command]
fn get_device_id() -> String {
    said_core::paths::device_id()
}

#[tauri::command]
fn get_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "desktop".to_string())
}

#[tauri::command]
async fn cloud_logout(backend: State<'_, BackendState>) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    api::clear_cloud_token(&ep).await
}

#[tauri::command]
async fn get_cloud_status(backend: State<'_, BackendState>) -> Result<api::CloudStatus, String> {
    let ep = get_endpoint(&backend)?;
    api::get_cloud_status(&ep).await
}

/// On launch, refresh license from cloud if a token is stored.
/// Returns the cached tier on network failure (graceful degradation).
#[tauri::command]
async fn refresh_license(backend: State<'_, BackendState>) -> Result<serde_json::Value, String> {
    let ep = get_endpoint(&backend)?;
    let status = api::get_cloud_status(&ep).await?;
    if !status.connected {
        return Ok(serde_json::json!({ "tier": "free", "source": "local" }));
    }
    // We don't store the raw token in Tauri state, but the backend has it.
    // We can get it back via the status endpoint... but the backend doesn't
    // expose the raw token over HTTP for security. So for license refresh,
    // Tauri asks the backend to re-check — the backend can do this if needed.
    // For now, return the locally-stored tier.
    Ok(serde_json::json!({
        "tier":      status.license_tier,
        "connected": status.connected,
        "source":    "local",
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn get_endpoint(backend: &State<'_, BackendState>) -> Result<BackendEndpoint, String> {
    let lock = backend.0.lock().map_err(|_| "lock failed")?;
    lock.clone().ok_or_else(|| "backend not started".into())
}

// ── Native notch HUD sidecar wiring ─────────────────────────────────────────
// `wire_notch_events` mirrors the status-bar event stream into the Swift HUD via
// in-process Rust listeners; `handle_notch_action` routes the HUD's user actions
// back through the same backend endpoints the React status bar uses.

/// Register Rust-side listeners that forward each status-bar event to the
/// sidecar. Payload field names already match the sidecar protocol (snake_case),
/// so most events are pass-through with a `type` tag added.
fn wire_notch_events(app: &tauri::AppHandle, sidecar: &notch_sidecar::NotchSidecar) {
    use tauri::Listener as _;

    // app-state carries the full snapshot; the HUD only needs the coarse state.
    {
        let sc = sidecar.clone();
        app.listen("app-state", move |e| {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(e.payload()) {
                if let Some(state) = v.get("state").and_then(|s| s.as_str()) {
                    sc.send(&serde_json::json!({ "type": "state", "kind": state }));
                }
            }
        });
    }

    // Pass-through events: forward the payload verbatim with a `type` tag.
    let passthrough: [(&str, &str); 13] = [
        ("voice-status", "status"),
        ("voice-done", "done"),
        ("voice-output", "output"),
        ("voice-error", "error"),
        ("vocab-learned", "learned"),
        ("email-learned", "email_saved"),
        ("vocab-queued", "queued"),
        ("vocab-confirm", "confirm"),
        ("vocab-review", "review"),
        ("vocab-negative", "negative_confirm"),
        ("vocab-wrong-fixed", "wrong_fixed"),
        ("status-bar-placement-mode", "placement"),
        ("auto-update-ready", "update_ready"),
    ];
    for (event, ty) in passthrough {
        let sc = sidecar.clone();
        app.listen(event, move |e| {
            let mut v: serde_json::Value =
                serde_json::from_str(e.payload()).unwrap_or_else(|_| serde_json::json!({}));
            if !v.is_object() {
                v = serde_json::json!({});
            }
            v["type"] = serde_json::Value::String(ty.to_string());
            sc.send(&v);
        });
    }
}

fn notch_str(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string()
}

/// Route a sidecar action back to the same backend endpoints the React status
/// bar invokes. Runs on the sidecar reader thread; HTTP work is spawned async.
fn handle_notch_action(app: &tauri::AppHandle, action: serde_json::Value) {
    let ty = action.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match ty {
        "ready" => tracing::info!("[notch] sidecar ready"),

        // confirm a single vocab term (learn | skip) → /v1/confirm-term
        "confirm" => {
            let endpoint = notch_endpoint(app);
            if let Some(ep) = endpoint {
                let term = notch_str(&action, "term");
                let original = notch_str(&action, "original");
                let decision = notch_str(&action, "decision");
                let recording_id = action
                    .get("recording_id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let body = serde_json::json!({
                        "term": term, "original": original,
                        "action": decision, "recording_id": recording_id,
                    });
                    let ok = reqwest::Client::new()
                        .post(format!("{}/v1/confirm-term", ep.url))
                        .header("Authorization", ep.bearer())
                        .json(&body)
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false);
                    if ok && decision == "learn" {
                        let _ = app.emit("vocabulary-changed", ());
                    }
                });
            }
        }

        // batch-learn reviewed corrections → api::confirm_batch
        "confirm_batch" => {
            let endpoint = notch_endpoint(app);
            if let Some(ep) = endpoint {
                let recording_id = action
                    .get("recording_id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let request_items: Vec<api::ConfirmBatchRequestItem> = action
                    .get("items")
                    .and_then(|i| i.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| {
                                let context = v
                                    .get("context")
                                    .and_then(|value| value.as_str())
                                    .map(|value| value.to_string());
                                let tag = v
                                    .get("tag")
                                    .and_then(|value| value.as_str())
                                    .map(|value| value.to_string());
                                Some(api::ConfirmBatchRequestItem {
                                    original: v.get("original")?.as_str()?.to_string(),
                                    corrected: v.get("corrected")?.as_str()?.to_string(),
                                    context,
                                    tag,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !request_items.is_empty() {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if api::confirm_batch(&ep, &request_items, recording_id.as_deref(), None)
                            .await
                            .is_ok()
                        {
                            let _ = app.emit("vocabulary-changed", ());
                        }
                    });
                }
            }
        }

        // stop a wrong correction → /v1/block-correction
        "block" => {
            let endpoint = notch_endpoint(app);
            if let Some(ep) = endpoint {
                let variant = notch_str(&action, "variant");
                let wrong = notch_str(&action, "wrong_replacement");
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let body =
                        serde_json::json!({ "variant": variant, "wrong_replacement": wrong });
                    let ok = reqwest::Client::new()
                        .post(format!("{}/v1/block-correction", ep.url))
                        .header("Authorization", ep.bearer())
                        .json(&body)
                        .send()
                        .await
                        .map(|r| r.status().is_success())
                        .unwrap_or(false);
                    if ok {
                        let _ = app.emit("vocabulary-changed", ());
                    }
                });
            }
        }

        // retry a failed dictation (honours the selected speech model)
        "retry" => {
            let audio_id = notch_str(&action, "audio_id");
            if !audio_id.is_empty() {
                let state = app.state::<SharedApp>();
                let backend = app.state::<BackendState>();
                let _ = retry_recording(audio_id, state, backend, app.clone());
            }
        }

        "open_audio" => {
            let audio_id = notch_str(&action, "audio_id");
            if !audio_id.is_empty() {
                if let Err(e) = reveal_saved_audio(audio_id.clone()) {
                    tracing::warn!("[notch] reveal saved audio failed audio_id={audio_id}: {e}");
                }
            }
        }

        "copy_details" => {
            let text = notch_str(&action, "text");
            if !text.is_empty() {
                if let Err(e) = copy_text_to_clipboard(&text) {
                    tracing::warn!("[notch] copy details failed: {e}");
                }
            }
        }

        // sidecar already collapsed locally; nothing to do server-side.
        "dismiss" => {}

        // undo / apply_update / snooze_update / copy_recent / reposition:
        // not yet wired to backend logic — logged for follow-up.
        other => tracing::info!("[notch] unhandled action: {other}"),
    }
}

fn notch_endpoint(app: &tauri::AppHandle) -> Option<BackendEndpoint> {
    let backend = app.state::<BackendState>();
    get_endpoint(&backend).ok()
}

fn said_log_dir() -> std::path::PathBuf {
    said_core::paths::log_dir()
}

fn read_recent_log(path: &std::path::Path, marker: &str) -> (String, bool) {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return ("".into(), false),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(DEBUG_LOG_MAX_BYTES);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return ("".into(), false);
    }

    let mut bytes = Vec::with_capacity((len - start).min(DEBUG_LOG_MAX_BYTES) as usize);
    if file.read_to_end(&mut bytes).is_err() {
        return ("".into(), false);
    }
    let mut text = String::from_utf8_lossy(&bytes).to_string();
    if !marker.is_empty()
        && let Some(idx) = text.rfind(marker)
    {
        text = text[idx..].to_string();
    }
    (text, start > 0)
}

#[tauri::command]
fn get_debug_logs() -> DebugLogs {
    let dir = said_log_dir();
    let desktop_path = dir.join("said.log");
    let backend_path = dir.join("backend.log");
    let breadcrumb_path = diag::breadcrumb_log_path();
    let (desktop, desktop_truncated) =
        read_recent_log(&desktop_path, "[main] said desktop starting");
    let (backend, backend_truncated) = read_recent_log(&backend_path, "airnote-backend build=");
    let (breadcrumbs, breadcrumbs_truncated) = read_recent_log(&breadcrumb_path, "");

    let combined = format!(
        "── AirNote desktop ({}) ──\n{}\n\n── airnote-backend ({}) ──\n{}\n\n── crash breadcrumbs ({}) ──\n{}",
        desktop_path.display(),
        if desktop.trim().is_empty() {
            "(no desktop log found)"
        } else {
            desktop.trim_end()
        },
        backend_path.display(),
        if backend.trim().is_empty() {
            "(no backend log found)"
        } else {
            backend.trim_end()
        },
        breadcrumb_path.display(),
        if breadcrumbs.trim().is_empty() {
            "(no crash breadcrumbs found)"
        } else {
            breadcrumbs.trim_end()
        },
    );

    DebugLogs {
        desktop_path: desktop_path.display().to_string(),
        backend_path: backend_path.display().to_string(),
        breadcrumb_path: breadcrumb_path.display().to_string(),
        desktop,
        backend,
        breadcrumbs,
        combined,
        truncated: desktop_truncated || backend_truncated || breadcrumbs_truncated,
    }
}

fn process_perf(pid: sysinfo::Pid, process: &sysinfo::Process) -> ProcessPerf {
    ProcessPerf {
        pid: pid.as_u32(),
        name: process.name().to_string_lossy().to_string(),
        cpu_percent: process.cpu_usage(),
        memory_bytes: process.memory(),
        virtual_memory_bytes: process.virtual_memory(),
    }
}

#[tauri::command]
fn get_performance_snapshot(
    perf: State<'_, PerformanceState>,
    backend_handle: State<'_, BackendHandleState>,
) -> Result<PerformanceSnapshot, String> {
    let mut sys = perf.0.lock().map_err(|_| "performance lock failed")?;

    let desktop_pid = sysinfo::Pid::from_u32(std::process::id());
    let owned_backend_pid = backend_handle
        .0
        .lock()
        .ok()
        .and_then(|handle| handle.as_ref().and_then(|h| h.pid()))
        .map(sysinfo::Pid::from_u32);

    sys.refresh_memory();
    sys.refresh_cpu_all();
    // Refresh only our two processes when the backend PID is known (the steady
    // state) instead of scanning the ENTIRE OS process table every 1.5s — that
    // full scan was a needless main-thread cost while the perf monitor is open.
    // Fall back to a full scan only when we must name-match the backend.
    if let Some(bp) = owned_backend_pid {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[desktop_pid, bp]), true);
    } else {
        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    }

    let desktop = sys
        .process(desktop_pid)
        .map(|process| process_perf(desktop_pid, process));

    let backend_pid = owned_backend_pid.or_else(|| {
        sys.processes()
            .iter()
            .filter(|(_, process)| {
                matches!(
                    process.name().to_string_lossy().trim_end_matches(".exe"),
                    "airnote-backend" | "said-backend"
                )
            })
            .max_by_key(|(_, process)| process.memory())
            .map(|(pid, _)| *pid)
    });
    let backend = backend_pid.and_then(|pid| sys.process(pid).map(|p| process_perf(pid, p)));

    Ok(PerformanceSnapshot {
        timestamp_ms: now_ms(),
        cpu_percent: sys.global_cpu_usage(),
        physical_core_count: sys.physical_core_count(),
        total_memory_bytes: sys.total_memory(),
        used_memory_bytes: sys.used_memory(),
        available_memory_bytes: sys.available_memory(),
        total_swap_bytes: sys.total_swap(),
        used_swap_bytes: sys.used_swap(),
        desktop,
        backend,
        gpu: GpuPerf {
            available: false,
            label: "GPU metrics unavailable from macOS user-space sampler".to_string(),
            utilization_percent: None,
            memory_bytes: None,
        },
    })
}

// ── Edit watcher ──────────────────────────────────────────────────────────────

const EDIT_WATCH_FAST_INTERVAL: Duration = Duration::from_millis(50);
const EDIT_WATCH_SLOW_INTERVAL: Duration = Duration::from_millis(200);
const EDIT_WATCH_BLOCKING_TIMEOUT: Duration = Duration::from_millis(500);
const EDIT_WATCH_CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(300);
const EDIT_WATCH_EMPTY_FIELD_BOUNDARY: Duration = Duration::from_millis(400);
const EDIT_WATCH_MAX_OBSERVATIONS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnedTextSpan {
    start_chars: usize,
    len_chars: usize,
    text: String,
    prefix: String,
    suffix: String,
}

#[derive(Debug, Clone)]
struct EditCaptureAnchor {
    target_pid: Option<i32>,
    field_fingerprint: Option<u64>,
    pre_paste_text: Option<String>,
    post_paste_text: String,
    owned_span: Option<OwnedTextSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EditObservation {
    elapsed_ms: u64,
    field_value: String,
    owned_text: String,
}

#[derive(Debug, Default)]
struct EditObservationTimeline {
    observations: std::collections::VecDeque<EditObservation>,
    total_observations: usize,
}

impl EditObservationTimeline {
    fn record(
        &mut self,
        anchor: &EditCaptureAnchor,
        post_paste: &str,
        field_value: &str,
        elapsed_ms: u64,
    ) -> Result<bool, &'static str> {
        if field_value == post_paste || field_value.is_empty() {
            return Ok(false);
        }
        let owned_text = extract_owned_text(anchor, field_value)?;
        if self
            .observations
            .back()
            .is_some_and(|last| last.field_value == field_value && last.owned_text == owned_text)
        {
            return Ok(false);
        }
        self.total_observations += 1;
        if self.observations.len() == EDIT_WATCH_MAX_OBSERVATIONS {
            self.observations.pop_front();
        }
        self.observations.push_back(EditObservation {
            elapsed_ms,
            field_value: field_value.to_string(),
            owned_text,
        });
        Ok(true)
    }

    fn latest_field_value(&self) -> Option<&str> {
        self.observations
            .back()
            .map(|observation| observation.field_value.as_str())
    }

    fn retained_count(&self) -> usize {
        self.observations.len()
    }

    fn dropped_count(&self) -> usize {
        self.total_observations
            .saturating_sub(self.observations.len())
    }

    fn elapsed_span_ms(&self) -> Option<u64> {
        let first = self.observations.front()?.elapsed_ms;
        let last = self.observations.back()?.elapsed_ms;
        Some(last.saturating_sub(first))
    }
}

fn effective_owned_field_value(
    anchor: &EditCaptureAnchor,
    last_value: &str,
    observations: &EditObservationTimeline,
) -> String {
    if !last_value.is_empty() && extract_owned_text(anchor, last_value).is_ok() {
        last_value.to_string()
    } else {
        observations
            .latest_field_value()
            .unwrap_or(last_value)
            .to_string()
    }
}

fn derive_owned_text_span(
    pre_paste: Option<&str>,
    post_paste: &str,
    polished: &str,
) -> Option<OwnedTextSpan> {
    if post_paste.is_empty() {
        return None;
    }

    if let Some(pre) = pre_paste {
        let prefix_bytes = common_prefix_bytes(pre, post_paste);
        let pre_rest = &pre[prefix_bytes..];
        let post_rest = &post_paste[prefix_bytes..];
        let suffix_bytes = common_suffix_bytes(pre_rest, post_rest);
        let inserted_end = post_paste.len().checked_sub(suffix_bytes)?;
        if inserted_end <= prefix_bytes {
            return None;
        }
        let text = post_paste[prefix_bytes..inserted_end].to_string();
        return Some(OwnedTextSpan {
            start_chars: post_paste[..prefix_bytes].chars().count(),
            len_chars: text.chars().count(),
            text,
            prefix: post_paste[..prefix_bytes].to_string(),
            suffix: post_paste[inserted_end..].to_string(),
        });
    }

    let polished = polished.trim();
    if polished.is_empty() {
        return None;
    }
    let mut matches = post_paste.match_indices(polished);
    let (offset, matched) = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(OwnedTextSpan {
        start_chars: post_paste[..offset].chars().count(),
        len_chars: matched.chars().count(),
        text: matched.to_string(),
        prefix: post_paste[..offset].to_string(),
        suffix: post_paste[offset + matched.len()..].to_string(),
    })
}

fn extract_owned_text(
    anchor: &EditCaptureAnchor,
    current_text: &str,
) -> Result<String, &'static str> {
    let span = anchor.owned_span.as_ref().ok_or("anchor_missing")?;
    let after_prefix = current_text
        .strip_prefix(&span.prefix)
        .ok_or("prefix_mismatch")?;
    let owned = after_prefix
        .strip_suffix(&span.suffix)
        .ok_or("suffix_mismatch")?;
    Ok(owned.trim().to_string())
}

fn capture_edit_anchor(
    target_pid: Option<i32>,
    pre_paste_text: Option<String>,
    polished: &str,
) -> EditCaptureAnchor {
    let post_paste_text = match target_pid {
        Some(pid) => paster::read_focused_value_fast_for_pid(pid),
        None => paster::read_focused_value_fast(),
    }
    .unwrap_or_default();
    #[cfg(target_os = "macos")]
    let field_fingerprint = target_pid.and_then(paster::focused_element_fingerprint_for_pid);
    #[cfg(not(target_os = "macos"))]
    let field_fingerprint = None;
    let owned_span = derive_owned_text_span(pre_paste_text.as_deref(), &post_paste_text, polished);
    EditCaptureAnchor {
        target_pid,
        field_fingerprint,
        pre_paste_text,
        post_paste_text,
        owned_span,
    }
}

#[cfg(target_os = "macos")]
async fn read_clipboard_text_readonly() -> Option<String> {
    let read = tokio::task::spawn_blocking(|| {
        let output = std::process::Command::new("pbpaste").output().ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8(output.stdout)
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    });
    tokio::time::timeout(EDIT_WATCH_CLIPBOARD_TIMEOUT, read)
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
}

#[cfg(not(target_os = "macos"))]
async fn read_clipboard_text_readonly() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn paste_shortcut_seen_since(since: std::time::Instant) -> bool {
    said_hotkey::key_buffer().lock().is_ok_and(|events| {
        events
            .iter()
            .any(|event| event.when >= since && matches!(event.evt, said_hotkey::KeyEvt::Paste))
    })
}

#[cfg(not(target_os = "macos"))]
fn paste_shortcut_seen_since(_since: std::time::Instant) -> bool {
    false
}

fn normalized_clipboard_candidate(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clipboard_content_was_added(polished: &str, user_kept: &str, clipboard: &str) -> bool {
    let polished = normalized_clipboard_candidate(polished);
    let kept = normalized_clipboard_candidate(user_kept);
    let clipboard = normalized_clipboard_candidate(clipboard);
    if clipboard.chars().count() < 4
        || polished.is_empty()
        || kept.is_empty()
        || clipboard == polished
        || !kept.contains(&clipboard)
    {
        return false;
    }

    let Some(offset) = kept.find(&clipboard) else {
        return false;
    };
    let clip_end = offset + clipboard.len();
    let is_surrounding_addition =
        kept[..offset].trim().is_empty() || kept[clip_end..].trim().is_empty();
    if !is_surrounding_addition {
        return false;
    }

    let without_clipboard = format!("{} {}", &kept[..offset], &kept[clip_end..]);
    let remaining = normalized_clipboard_candidate(&without_clipboard);
    remaining == polished || shares_word_overlap_ratio(&remaining, &polished) >= 0.6
}

fn shares_word_overlap_ratio(candidate: &str, reference: &str) -> f64 {
    let candidate_words: std::collections::HashSet<String> = candidate
        .split_whitespace()
        .map(alnum_word_core)
        .filter(|word| word.chars().count() > 2)
        .map(str::to_lowercase)
        .collect();
    let reference_words: std::collections::HashSet<String> = reference
        .split_whitespace()
        .map(alnum_word_core)
        .filter(|word| word.chars().count() > 2)
        .map(str::to_lowercase)
        .collect();
    if reference_words.is_empty() {
        return 0.0;
    }
    let shared = reference_words
        .iter()
        .filter(|word| candidate_words.contains(*word))
        .count();
    shared as f64 / reference_words.len() as f64
}

/// Compute edit-watch timeouts scaled by sentence length.
///
/// There are two different idle windows:
/// - before an edit, keep the watcher cheap and finish quickly if the user
///   accepts the paste unchanged;
/// - after the first edit, wait much longer after the *last* field change so
///   a user can read the whole sentence and make several corrections before a
///   single classify/review-card pass fires.
fn edit_watch_timeouts(word_count: usize) -> (Duration, Duration, Duration) {
    let words = word_count.max(5).min(80) as f64;
    // No-edit idle: this is the user's reading window before they make the
    // first correction. Short 6-12s windows miss the normal flow where someone
    // reads the whole sentence, spots terms to fix, then starts editing.
    let no_edit_idle_secs = (20.0 + (words - 15.0).max(0.0) * 0.20).min(45.0);

    // Post-edit idle: users often correct the first visible error, pause to
    // read the rest, then fix more terms. Keep this comfortably in the 10-15s
    // range even for short sentences, and slightly longer for long paragraphs.
    let post_edit_idle_secs = (18.0 + (words - 15.0).max(0.0) * 0.14).min(35.0);

    // Hard guard only. After an edit we still prefer the post-edit idle window
    // over an early max-duration cut, so the loop uses a longer hard cap.
    let max_secs = (30.0 + (words - 15.0).max(0.0) * 0.7).min(90.0);

    (
        Duration::from_secs_f64(max_secs),
        Duration::from_secs_f64(no_edit_idle_secs),
        Duration::from_secs_f64(post_edit_idle_secs),
    )
}

async fn blocking_ax_option<T, F>(label: &'static str, f: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> Option<T> + Send + 'static,
{
    match tokio::time::timeout(EDIT_WATCH_BLOCKING_TIMEOUT, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(value)) => value,
        Ok(Err(err)) => {
            tracing::warn!("[edit-watch] blocking AX task {label} failed: {err}");
            None
        }
        Err(_) => {
            tracing::warn!("[edit-watch] blocking AX task {label} timed out");
            None
        }
    }
}

async fn cancellable_sleep(token: &CancellationToken, duration: Duration) -> bool {
    tokio::select! {
        _ = token.cancelled() => false,
        _ = tokio::time::sleep(duration) => true,
    }
}

fn cancel_edit_watcher(app: &tauri::AppHandle, reason: &str) {
    let st = app.state::<EditWatcherState>();
    let mut guard = match st.0.lock() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("[edit-watch] watcher state lock poisoned; recovering");
            e.into_inner()
        }
    };
    if let Some(prev) = guard.take() {
        tracing::info!("[edit-watch] cancelling watcher — {reason}");
        prev.cancel.cancel();
    }
}

fn finalize_edit_watcher(app: &tauri::AppHandle, reason: &str) {
    let st = app.state::<EditWatcherState>();
    let mut guard = match st.0.lock() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("[edit-watch] watcher state lock poisoned; recovering");
            e.into_inner()
        }
    };
    if let Some(prev) = guard.take() {
        tracing::info!("[edit-watch] finalizing watcher — {reason}");
        prev.finalize.cancel();
    }
}

fn start_edit_watcher(
    back_arc: Arc<Mutex<Option<BackendEndpoint>>>,
    app: tauri::AppHandle,
    client_run_id: Option<String>,
    recording_id: String,
    polished: String,
    watch_start: std::time::Instant,
    anchor: EditCaptureAnchor,
) {
    let control = {
        let st = app.state::<EditWatcherState>();
        let mut guard = match st.0.lock() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("[edit-watch] watcher state lock poisoned; recovering");
                e.into_inner()
            }
        };
        if let Some(prev) = guard.take() {
            tracing::info!("[edit-watch] cancelling previous watcher");
            prev.cancel.cancel();
            std::thread::yield_now();
        }
        let control =
            new_edit_watcher_control(EDIT_WATCHER_GENERATION.fetch_add(1, Ordering::Relaxed) + 1);
        *guard = Some(control.clone());
        control
    };

    let app_for_task = app.clone();
    let task_generation = control.generation;
    tauri::async_runtime::spawn(async move {
        watch_for_edit(
            control.cancel.clone(),
            control.finalize.clone(),
            back_arc,
            app_for_task.clone(),
            client_run_id,
            recording_id,
            polished,
            watch_start,
            anchor,
        )
        .await;

        if let Ok(mut guard) = app_for_task.state::<EditWatcherState>().0.lock() {
            if edit_watcher_generation_is_current(guard.as_ref(), task_generation) {
                *guard = None;
            }
        }
    });
}

/// After pasting, poll the focused text element through the sentence-scaled reading
/// window. Once editing starts, allow up to 120 seconds total and wait for the
/// sentence-scaled post-edit idle period before classifying the owned field value.
async fn watch_for_edit(
    cancel_token: CancellationToken,
    finalize_token: CancellationToken,
    back_arc: Arc<Mutex<Option<BackendEndpoint>>>,
    app: tauri::AppHandle,
    client_run_id: Option<String>,
    recording_id: String,
    polished: String,                // the AI-generated text we pasted
    watch_start: std::time::Instant, // captured at the call site, right after paste
    mut anchor: EditCaptureAnchor,
) {
    use std::time::Instant;

    let clipboard_at_watch_start = read_clipboard_text_readonly().await;

    // Let the paste animation settle and focus move into the text field.
    // 400ms covers paste animation (~200ms) + AX cache start; retries handle the rest.
    if !cancellable_sleep(&cancel_token, Duration::from_millis(400)).await {
        tracing::info!("[edit-watch] watcher cancelled before start for {recording_id}");
        return;
    }

    // Prefer the PID captured when recording started. That is the key
    // OpenWhispr-style fix: we monitor the app the user dictated into, not
    // whatever happens to be frontmost after our HUD/status UI has updated.
    let focused_pid_after_paste =
        blocking_ax_option("focused_pid after-paste", paster::focused_pid).await;
    let initial_pid = anchor.target_pid.or(focused_pid_after_paste);

    // Attempt to get the initial field value.  Chrome / Electron may still be
    // building their AX cache even after the pre-unlock at recording-start, so
    // we retry a few times with increasing delays before declaring "AX blind".
    let post_paste = {
        let mut val = anchor.post_paste_text.clone();
        if val.is_empty() {
            val = blocking_ax_option(
                "read_focused_value_first initial",
                move || match initial_pid {
                    Some(pid) => paster::read_focused_value_first_for_pid(pid),
                    None => paster::read_focused_value_first(),
                },
            )
            .await
            .unwrap_or_default();
        }
        if val.is_empty() {
            // 2nd attempt after 300 ms
            if !cancellable_sleep(&cancel_token, Duration::from_millis(300)).await {
                tracing::info!(
                    "[edit-watch] watcher cancelled during initial retry for {recording_id}"
                );
                return;
            }
            val = blocking_ax_option(
                "read_focused_value_first retry1",
                move || match initial_pid {
                    Some(pid) => paster::read_focused_value_first_for_pid(pid),
                    None => paster::read_focused_value_first(),
                },
            )
            .await
            .unwrap_or_default();
        }
        if val.is_empty() {
            // 3rd attempt after another 500 ms — AX tree should be ready by now
            if !cancellable_sleep(&cancel_token, Duration::from_millis(500)).await {
                tracing::info!(
                    "[edit-watch] watcher cancelled during initial retry for {recording_id}"
                );
                return;
            }
            val = blocking_ax_option(
                "read_focused_value_first retry2",
                move || match initial_pid {
                    Some(pid) => paster::read_focused_value_first_for_pid(pid),
                    None => paster::read_focused_value_first(),
                },
            )
            .await
            .unwrap_or_default();
        }
        val
    };
    if anchor.post_paste_text.is_empty() && !post_paste.is_empty() {
        anchor.post_paste_text = post_paste.clone();
        anchor.owned_span =
            derive_owned_text_span(anchor.pre_paste_text.as_deref(), &post_paste, &polished);
        #[cfg(target_os = "macos")]
        if anchor.field_fingerprint.is_none() {
            anchor.field_fingerprint =
                initial_pid.and_then(paster::focused_element_fingerprint_for_pid);
        }
    }

    let mut last_val = post_paste.clone();
    let mut observations = EditObservationTimeline::default();
    let mut idle_at = Instant::now();
    let started = Instant::now();
    let mut last_change_at = Instant::now();
    let mut current_interval = EDIT_WATCH_FAST_INTERVAL;
    let mut last_pid = initial_pid;
    let mut ownership_lost_reason: Option<&'static str> = None;
    let mut finalize_requested = finalize_token.is_cancelled();
    let mut field_empty_since: Option<Instant> = None;
    let mut explicit_boundary: Option<&'static str> = None;

    // Scale timeouts by sentence length — long sentences need more reading time
    let word_count = polished.split_whitespace().count();
    let (max_duration, no_edit_idle_timeout, post_edit_idle_timeout) =
        edit_watch_timeouts(word_count);
    tracing::info!(
        "[edit-watch] word_count={word_count} max={}s no_edit_idle={}s post_edit_idle={}s",
        max_duration.as_secs(),
        no_edit_idle_timeout.as_secs(),
        post_edit_idle_timeout.as_secs(),
    );
    let mut saw_user_edit = false;
    // Capture-error metadata, hoisted so we can ship it to the backend's
    // CAPTURE_ERROR pre-filter alongside the edit text.
    let mut app_switched_during_capture: bool = false;

    tracing::info!(
        "[edit-watch] watching {recording_id} — target_pid={:?} focused_after_paste={:?} initial field readable: {} (len={})",
        anchor.target_pid,
        focused_pid_after_paste,
        !post_paste.is_empty(),
        post_paste.len(),
    );

    // Poll loop: adaptive cadence. No mid-loop side effects: AX and pasteboard
    // reads are non-mutating, and app/field/submit boundaries close the session.
    while !finalize_requested {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                tracing::info!("[edit-watch] watcher cancelled for {recording_id}");
                return;
            }
            _ = finalize_token.cancelled() => {
                finalize_requested = true;
                tracing::info!("[edit-watch] finalize requested for {recording_id}");
                break;
            }
            _ = tokio::time::sleep(current_interval) => {}
        }

        // Leaving the target app closes this edit session. The last owned value
        // remains valid, but text entered after the switch cannot belong to it.
        let now_pid = blocking_ax_option("focused_pid poll", paster::focused_pid).await;
        let pid_switched = edit_watch_crossed_app_boundary(initial_pid, now_pid);
        if pid_switched {
            app_switched_during_capture = true;
            explicit_boundary = Some("app_switched");
            tracing::info!(
                "[edit-watch] app boundary for {recording_id} — initial_pid={initial_pid:?} now_pid={now_pid:?}"
            );
            break;
        }

        // Detect if target app exited — stop polling a dead process
        if let Some(pid) = initial_pid {
            if now_pid.is_none() || now_pid == Some(1) {
                tracing::info!(
                    "[edit-watch] target process pid={pid} appears dead — finalizing early"
                );
                break;
            }
        }

        // Read the current field value from the locked target app when we have
        // one. Measure AX latency for adaptive polling.
        let ax_read_start = std::time::Instant::now();
        // This avoids accidentally sampling our HUD or a newly focused app.
        let now_val = if let Some(pid) = initial_pid {
            let fast = blocking_ax_option("read_focused_value_fast target-pid poll", move || {
                paster::read_focused_value_fast_for_pid(pid)
            })
            .await;
            if fast.as_ref().is_some_and(|v| !v.is_empty()) || post_paste.is_empty() {
                fast
            } else {
                blocking_ax_option("read_focused_value_first target-pid fallback", move || {
                    paster::read_focused_value_first_for_pid(pid)
                })
                .await
            }
        } else if now_pid != last_pid {
            last_pid = now_pid;
            blocking_ax_option(
                "read_focused_value_first focus-change",
                paster::read_focused_value_first,
            )
            .await
        } else {
            blocking_ax_option(
                "read_focused_value_fast poll",
                paster::read_focused_value_fast,
            )
            .await
        }
        .unwrap_or_default();
        let ax_latency = ax_read_start.elapsed();
        if ax_latency > Duration::from_millis(100) && current_interval < Duration::from_millis(200)
        {
            current_interval = Duration::from_millis(ax_latency.as_millis() as u64 * 2);
            tracing::debug!(
                "[edit-watch] AX slow ({}ms) — adapting poll to {}ms",
                ax_latency.as_millis(),
                current_interval.as_millis(),
            );
        }
        if now_val != last_val {
            #[cfg(target_os = "macos")]
            if let (Some(pid), Some(expected)) = (initial_pid, anchor.field_fingerprint) {
                let observed = blocking_ax_option("field fingerprint after change", move || {
                    paster::focused_element_fingerprint_for_pid(pid)
                })
                .await;
                if observed != Some(expected) {
                    explicit_boundary = Some("field_changed");
                    tracing::info!(
                        "[edit-watch] field boundary for {recording_id} — finalizing last owned state"
                    );
                    break;
                }
            }
            idle_at = Instant::now();
            last_change_at = Instant::now();
            current_interval = EDIT_WATCH_FAST_INTERVAL;
            field_empty_since = now_val.is_empty().then(Instant::now);
            if now_val != post_paste {
                if !saw_user_edit {
                    tracing::info!(
                        "[edit-watch] first edit observed for {recording_id}; waiting {}s after the last change before classify",
                        post_edit_idle_timeout.as_secs()
                    );
                }
                saw_user_edit = true;
            }
            if let Err(reason) = observations.record(
                &anchor,
                &post_paste,
                &now_val,
                started.elapsed().as_millis() as u64,
            ) {
                ownership_lost_reason = Some(reason);
                tracing::info!(
                    "[edit-watch] ownership lost for {recording_id} — {reason}; refusing changed field state"
                );
                break;
            }
            last_val = now_val;
        } else if last_change_at.elapsed() > Duration::from_secs(2) {
            current_interval = EDIT_WATCH_SLOW_INTERVAL;
        }

        if field_empty_since
            .is_some_and(|empty_since| empty_since.elapsed() >= EDIT_WATCH_EMPTY_FIELD_BOUNDARY)
        {
            explicit_boundary = Some("field_cleared");
            tracing::info!("[edit-watch] field-clear boundary for {recording_id}");
            break;
        }

        let active_idle_timeout = if saw_user_edit {
            post_edit_idle_timeout
        } else {
            no_edit_idle_timeout
        };
        let hard_timeout = if saw_user_edit {
            Duration::from_secs(120)
        } else {
            max_duration
        };
        let done_by_idle = idle_at.elapsed() > active_idle_timeout;
        let done_by_hard_timeout = started.elapsed() > hard_timeout;
        let done = done_by_idle || done_by_hard_timeout;

        if done {
            tracing::info!(
                "[edit-watch] finishing watch for {recording_id}; reason={} total={}ms idle={}ms edited={}",
                if done_by_idle {
                    if saw_user_edit {
                        "post_edit_idle"
                    } else {
                        "no_edit_idle"
                    }
                } else {
                    "hard_timeout"
                },
                started.elapsed().as_millis(),
                idle_at.elapsed().as_millis(),
                saw_user_edit,
            );
            break;
        }
    }

    if finalize_requested {
        explicit_boundary = Some("new_recording");
        let final_value = blocking_ax_option("finalize current value", move || match initial_pid {
            Some(pid) => paster::read_focused_value_fast_for_pid(pid),
            None => paster::read_focused_value_fast(),
        })
        .await;
        if let Some(now_val) = final_value {
            let mut final_field_is_owned = true;
            #[cfg(target_os = "macos")]
            if let (Some(pid), Some(expected)) = (initial_pid, anchor.field_fingerprint) {
                let observed = blocking_ax_option("finalize field fingerprint", move || {
                    paster::focused_element_fingerprint_for_pid(pid)
                })
                .await;
                if observed != Some(expected) {
                    explicit_boundary = Some("field_changed");
                    final_field_is_owned = false;
                }
            }
            if final_field_is_owned && ownership_lost_reason.is_none() && now_val != last_val {
                saw_user_edit = now_val != post_paste;
                if let Err(reason) = observations.record(
                    &anchor,
                    &post_paste,
                    &now_val,
                    started.elapsed().as_millis() as u64,
                ) {
                    ownership_lost_reason = Some(reason);
                }
                last_val = now_val;
            }
        }
    }

    if let Some(reason) = ownership_lost_reason {
        if let (Some(run_id), Some(ep)) = (
            client_run_id.as_deref(),
            back_arc.lock().ok().and_then(|guard| guard.clone()),
        ) {
            telemetry::on_edit_excluded(&ep, run_id, reason);
        }
        return;
    }

    // A submit can clear the field after several valid edits. Recover the latest
    // anchored state rather than guessing from word overlap or a single snapshot.
    let effective_val = effective_owned_field_value(&anchor, &last_val, &observations);
    if effective_val != last_val {
        tracing::info!(
            "[edit-watch] final field unavailable (sent message?); using latest owned observation"
        );
    }

    let final_front_pid = blocking_ax_option("focused_pid final", paster::focused_pid).await;
    tracing::info!(
        "[edit-watch] done watching {recording_id} — field changed: {}, target still frontmost: {}, boundary={:?}",
        effective_val != post_paste,
        matches!((initial_pid, final_front_pid), (Some(a), Some(b)) if a == b),
        explicit_boundary,
    );

    // ── Determine user_kept + capture_method ───────────────────────────────────
    //
    // The capture_method is propagated to the backend so auto-promotion thresholds
    // can scale with capture confidence:
    //   • "ax"                 → AX API read directly (high confidence, ground truth)
    //   • "keystroke_verified" → keystroke replay AGREES with clipboard read (high)
    //   • "clipboard"          → clipboard read; keystroke unavailable or disagreed (medium)
    //   • "keystroke_only"     → keystroke replay; clipboard unreachable (LOW — pending only)

    let user_kept: String;
    let capture_method: &'static str;

    if !post_paste.is_empty() {
        // ── AX was readable — compare values directly ──────────────────────────
        if effective_val == post_paste {
            tracing::info!("[edit-watch] ax_no_edit for {recording_id}");
            if let (Some(run_id), Some(ep)) = (
                client_run_id.as_deref(),
                back_arc.lock().ok().and_then(|g| g.clone()),
            ) {
                telemetry::on_accepted_no_edit(&ep, run_id);
            }
            return;
        }
        user_kept = match extract_owned_text(&anchor, &effective_val) {
            Ok(text) => text,
            Err(reason) => {
                tracing::info!(
                    "[edit-watch] ownership lost for {recording_id} — {reason}; skipping classification"
                );
                if let (Some(run_id), Some(ep)) = (
                    client_run_id.as_deref(),
                    back_arc.lock().ok().and_then(|guard| guard.clone()),
                ) {
                    telemetry::on_edit_excluded(&ep, run_id, reason);
                }
                return;
            }
        };
        capture_method = "ax";
        tracing::info!(
            "[edit-watch] ax_capture for {recording_id}: {:?} → {:?}",
            polished.chars().take(60).collect::<String>(),
            user_kept.chars().take(60).collect::<String>(),
        );
    } else {
        // ── AX blind (Lark, Chrome contenteditable, WebView) ─────────────────
        //
        // Safe-learning policy: automatic learning must be read-only and must
        // never disturb clipboard, selection, focus, or typed input. Earlier
        // builds used Cmd+A/C clipboard capture here; that felt autonomous in
        // production apps, so AX-blind learning is intentionally skipped.
        tracing::info!(
            "[edit-watch] ax_unreadable_skip for {recording_id} — no clipboard or selection fallback"
        );
        if let (Some(run_id), Some(ep)) = (
            client_run_id.as_deref(),
            back_arc.lock().ok().and_then(|g| g.clone()),
        ) {
            telemetry::on_accepted_no_edit(&ep, run_id);
        }
        return;
    }

    // ── Pre-flight gates (cheap, no API call) ─────────────────────────────────

    if user_kept.is_empty() || user_kept.trim() == polished.trim() {
        tracing::info!("[edit-watch] no diff for {recording_id} — skipping");
        if let (Some(run_id), Some(ep)) = (
            client_run_id.as_deref(),
            back_arc.lock().ok().and_then(|g| g.clone()),
        ) {
            telemetry::on_edit_outcome(
                &ep,
                run_id,
                &polished,
                &user_kept,
                true,
                user_kept.is_empty(),
                true,
            );
        }
        return;
    }

    // Garbage check: if user_kept shares zero words with polished it's likely
    // a UI placeholder (e.g. Slack's "Type / for commands") that leaked through.
    //
    // Exception: format transformations like "abhishek at the rate gmail dot com"
    // → "abhishek@gmail.com" produce no word overlap but ARE valid corrections.
    // Detect these by checking if user_kept looks like an email, URL, handle, or
    // other compact identifier format — let those through to the classifier.
    if !shares_word_overlap(&user_kept, &polished) && !is_format_transformation(&user_kept) {
        tracing::info!(
            "[edit-watch] user_kept has no word overlap with polished — garbage, skipping. kept={:?}",
            user_kept.chars().take(40).collect::<String>()
        );
        if let (Some(run_id), Some(ep)) = (
            client_run_id.as_deref(),
            back_arc.lock().ok().and_then(|g| g.clone()),
        ) {
            telemetry::on_accepted_no_edit(&ep, run_id);
        }
        return;
    }

    // Whitespace / punctuation / AX-jitter filter (no API call needed).
    if !is_meaningful_edit(&polished, &user_kept) {
        tracing::info!("[edit-watch] edit not meaningful for {recording_id} — skipping");
        if let (Some(run_id), Some(ep)) = (
            client_run_id.as_deref(),
            back_arc.lock().ok().and_then(|g| g.clone()),
        ) {
            telemetry::on_accepted_no_edit(&ep, run_id);
        }
        return;
    }

    // ── Three-way classifier (Groq LLM call) ────────────────────────────────
    // Sends (recording_id, ai_output, user_kept) to the backend which looks up
    // the original transcript and asks Groq: "Is this an AI mistake correction
    // that we should learn from, or just user rephrasing / adding context?"
    tracing::info!(
        "[edit-watch] classifying edit for {recording_id}: polished={:?} → kept={:?}",
        polished.chars().take(50).collect::<String>(),
        user_kept.chars().take(50).collect::<String>(),
    );

    let ep_opt = back_arc.lock().ok().and_then(|g| g.clone());
    if let Some(ref ep) = ep_opt {
        let clipboard_at_capture = read_clipboard_text_readonly().await;
        let paste_shortcut_seen = paste_shortcut_seen_since(watch_start);
        let clipboard_addition_seen = clipboard_at_watch_start
            .as_deref()
            .is_some_and(|clipboard| clipboard_content_was_added(&polished, &user_kept, clipboard))
            || clipboard_at_capture.as_deref().is_some_and(|clipboard| {
                clipboard_content_was_added(&polished, &user_kept, clipboard)
            });
        let matches_clipboard = paste_shortcut_seen || clipboard_addition_seen;
        if matches_clipboard {
            tracing::info!(
                "[edit-watch] follow-up paste detected for {recording_id}; shortcut={} clipboard_addition={}",
                paste_shortcut_seen,
                clipboard_addition_seen,
            );
        }
        let capture_meta = api::CaptureMeta {
            time_since_paste_ms: watch_start.elapsed().as_millis() as u64,
            app_switched: app_switched_during_capture,
            matches_clipboard,
        };
        let mut edit_trace = said_core::dictation_trace::DictationTrace::default();
        edit_trace.add_stage(said_core::dictation_trace::TraceStageInput {
            stage: "edit_watch.capture",
            component: "desktop",
            function: "watch_for_edit",
            input: Some(&polished),
            output: Some(&user_kept),
            reason: Some("focused-field value captured after paste"),
            risk: Some("edit_capture"),
            metadata: serde_json::json!({
                "capture_method": capture_method,
                "time_since_paste_ms": capture_meta.time_since_paste_ms,
                "app_switched": capture_meta.app_switched,
                "matches_clipboard": capture_meta.matches_clipboard,
                "post_paste_readable": !post_paste.is_empty(),
                "saw_user_edit": saw_user_edit,
                "initial_pid": initial_pid,
                "final_front_pid": final_front_pid,
                "boundary": explicit_boundary,
                "field_fingerprint": anchor.field_fingerprint,
                "owned_span_start_chars": anchor.owned_span.as_ref().map(|span| span.start_chars),
                "owned_span_len_chars": anchor.owned_span.as_ref().map(|span| span.len_chars),
                "observation_count": observations.total_observations,
                "retained_observation_count": observations.retained_count(),
                "dropped_observation_count": observations.dropped_count(),
                "observation_span_ms": observations.elapsed_span_ms(),
            }),
            ..Default::default()
        });
        match api::classify_edit(
            ep,
            &recording_id,
            &polished,
            &user_kept,
            capture_method,
            capture_meta,
            client_run_id.as_deref(),
            anchor.pre_paste_text.as_deref(),
            Some(edit_trace.into_value()),
        )
        .await
        {
            Ok(resp) => {
                tracing::info!(
                    "[edit-watch] classify_result class={} promoted={} repeat={} learned={} notify={} reason={:?} pending={:?}",
                    resp.class,
                    resp.promoted_count,
                    resp.is_repeat,
                    resp.learned,
                    resp.notify,
                    resp.reason,
                    resp.pending_id
                );

                if let Some(run_id) = client_run_id.as_deref() {
                    telemetry::on_edit_outcome(
                        ep,
                        run_id,
                        &polished,
                        &user_kept,
                        false,
                        user_kept.trim().is_empty(),
                        false,
                    );
                    telemetry::on_classify_result(ep, run_id, &resp);
                }

                if let Some(email) = resp.learned_emails.first() {
                    if !email.trim().is_empty() {
                        let _ = present_status_bar_native(&app, "email-learned", false);
                        let _ = app.emit(
                            "email-learned",
                            serde_json::json!({
                                "email": email,
                                "message": "Email saved for next time",
                            }),
                        );
                    }
                }

                if (resp.notify || resp.learned) && resp.learned_emails.is_empty() {
                    let first_term = resp.promoted_terms.first().cloned();
                    if resp.notify {
                        if let Some(ref term) = first_term {
                            if !term.trim().is_empty() {
                                let _ = app.emit(
                                    "vocab-toast",
                                    serde_json::json!({
                                        "kind":   "added",
                                        "term":   term,
                                        "source": "auto",
                                    }),
                                );
                            }
                        }
                    }
                    // Show learning result in the status bar
                    let term_display = first_term
                        .clone()
                        .unwrap_or_else(|| "your correction".to_string());
                    let msg = match (resp.class.as_str(), resp.is_repeat) {
                        ("STT_ERROR" | "stt_error", true) => {
                            format!("Added new spelling for \"{}\"", term_display)
                        }
                        ("STT_ERROR" | "stt_error", false) => {
                            format!("Will recognise \"{}\" next time", term_display)
                        }
                        ("POLISH_ERROR" | "polish_error", _) => {
                            "Updated writing preference".to_string()
                        }
                        _ => "Remembered your correction".to_string(),
                    };
                    let _ = present_status_bar_native(&app, "vocab-learned", false);
                    let _ = app.emit(
                        "vocab-learned",
                        serde_json::json!({
                            "term": term_display,
                            "message": msg,
                        }),
                    );
                }

                // Surface queued terms in the status bar pill so the user knows
                // the system noticed and how many more edits are needed.
                if let Some(qt) = resp.queued_terms.first() {
                    if !qt.term.trim().is_empty() {
                        let remaining = qt.k - qt.sighting_count;
                        let _ = present_status_bar_native(&app, "vocab-queued", false);
                        let _ = app.emit(
                            "vocab-queued",
                            serde_json::json!({
                                "term": qt.term,
                                "remaining": remaining,
                                "sighting_count": qt.sighting_count,
                                "k": qt.k,
                            }),
                        );
                        tracing::info!(
                            "[edit-watch] queued {:?} — {}/{} sightings, {} more to learn",
                            qt.term,
                            qt.sighting_count,
                            qt.k,
                            remaining,
                        );
                    }
                }

                // Review candidates — show interactive picker
                if !resp.review_candidates.is_empty() {
                    let _ = present_status_bar_native(&app, "vocab-review", false);
                    let candidates: Vec<serde_json::Value> = resp
                        .review_candidates
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "original": c.original,
                                "corrected": c.corrected,
                                "term_type": c.term_type,
                                "learnable": c.learnable,
                                "tag": c.tag,
                                "context": c.context,
                            })
                        })
                        .collect();
                    let _ = app.emit(
                        "vocab-review",
                        serde_json::json!({
                            "candidates": candidates,
                            "detected_changes": &resp.changes,
                            "review_session_id": &resp.review_session_id,
                            "recording_id": recording_id,
                        }),
                    );
                    tracing::info!(
                        "[edit-watch] review card: {} candidate(s), {} detected change(s)",
                        resp.review_candidates.len(),
                        resp.changes.len(),
                    );
                }

                // Ambiguous terms — show confirmation toast in status bar
                for amb in &resp.ambiguous_terms {
                    let _ = present_status_bar_native(&app, "vocab-confirm", false);
                    let _ = app.emit(
                        "vocab-confirm",
                        serde_json::json!({
                            "term": amb.corrected,
                            "original": amb.original,
                            "context": amb.context,
                            "recording_id": amb.recording_id,
                        }),
                    );
                    tracing::info!(
                        "[edit-watch] asking user: {:?} → {:?} — ambiguous",
                        amb.original,
                        amb.corrected,
                    );
                }

                // Wrong corrections auto-fixed — show acknowledgement pill
                for neg in &resp.negative_terms {
                    let _ = present_status_bar_native(&app, "vocab-wrong-fixed", false);
                    let _ = app.emit(
                        "vocab-wrong-fixed",
                        serde_json::json!({
                            "term": neg.term,
                            "wrong_replacement": neg.wrong_replacement,
                        }),
                    );
                    tracing::info!(
                        "[edit-watch] wrong correction fixed: {:?} → {:?} — alias deleted, retraining",
                        neg.wrong_replacement,
                        neg.term,
                    );
                }

                if resp.learned || resp.pending_id.is_some() {
                    let _ = app.emit("pending-edits-changed", ());
                }

                // Poll the backend for retrain lifecycle and emit real events.
                if resp.learned {
                    let app_retrain = app.clone();
                    let ep_retrain = ep.clone();
                    tokio::spawn(async move {
                        let baseline_finished = api::get_retrain_status(&ep_retrain)
                            .await
                            .map(|s| s.finished_at)
                            .unwrap_or(0);

                        let mut started_emitted = false;
                        for _ in 0..30 {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            let Ok(status) = api::get_retrain_status(&ep_retrain).await else {
                                continue;
                            };

                            if status.running && !started_emitted {
                                started_emitted = true;
                                let _ = present_status_bar_native(
                                    &app_retrain,
                                    "retrain-started",
                                    false,
                                );
                                let _ = app_retrain.emit(
                                    "retrain-status",
                                    serde_json::json!({ "phase": "started" }),
                                );
                                tracing::info!("[retrain-poll] training started — notified UI");
                            }

                            if status.finished_at > baseline_finished {
                                let dur = status.duration_ms as f64 / 1000.0;
                                let _ =
                                    present_status_bar_native(&app_retrain, "retrain-done", false);
                                let _ = app_retrain.emit(
                                    "retrain-status",
                                    serde_json::json!({
                                        "phase": "done",
                                        "duration_s": dur,
                                        "success": status.success,
                                    }),
                                );
                                tracing::info!(
                                    "[retrain-poll] training finished in {dur:.1}s success={} — notified UI",
                                    status.success,
                                );
                                break;
                            }
                        }
                        // If retrain never started (API unavailable, feature gated, etc.)
                        // emit unavailable so the frontend doesn't stay in a silent state.
                        if !started_emitted {
                            tracing::info!(
                                "[retrain-poll] training did not start within 30 s (API unavailable or feature not enabled on this machine)"
                            );
                            let _ = app_retrain.emit(
                                "retrain-status",
                                serde_json::json!({ "phase": "unavailable" }),
                            );
                        }
                    });
                }

                // Local ASR does not have a live provider-bias channel. Learned
                // terms stay in the vocabulary/profile layer used by polish.
                if resp.learned && resp.promoted_count > 0 {
                    tracing::info!(
                        "[vocabulary] learned {} promoted term(s); polish profile will pick them up",
                        resp.promoted_count,
                    );
                }
            }
            Err(e) => {
                tracing::warn!("[edit-watch] classify_edit call failed: {e}");
                // Classifier unavailable — fail open (don't store, don't notify).
            }
        }
    }
    let _ = back_arc; // keep arc alive until end of scope
}

/// Returns true if `candidate` shares at least one significant word (>3 chars,
/// case-insensitive ASCII) with `reference`.  Used to detect when the app has
/// cleared its text field (e.g. Slack post-send shows "Type / for commands").
/// Returns true if `text` looks like a format-transformed value — an email,
/// URL, handle, phone number, or similar compact identifier.  These are valid
/// corrections that the word-overlap garbage gate would otherwise discard,
/// because "abhishek@gmail.com" shares no whitespace-delimited tokens with
/// "Abhishek at the rate gmail dot com."
fn is_format_transformation(text: &str) -> bool {
    let t = text.trim();
    // Email address: something@something.tld
    if t.contains('@') && t.contains('.') && !t.contains(' ') {
        return true;
    }
    // URL: starts with http/https/www or contains ://
    if t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("www.")
        || t.contains("://")
    {
        return true;
    }
    // Handle / username: starts with @ or contains _ with no spaces
    if t.starts_with('@') || (t.contains('_') && !t.contains(' ') && t.len() < 40) {
        return true;
    }
    // Phone number: mostly digits, spaces/dashes/dots/parens, 7+ chars
    let digits: usize = t.chars().filter(|c| c.is_ascii_digit()).count();
    if digits >= 7
        && t.chars()
            .all(|c| c.is_ascii_digit() || " -.+()\u{00A0}".contains(c))
    {
        return true;
    }
    false
}

fn shares_word_overlap(candidate: &str, reference: &str) -> bool {
    let cand_lower = candidate.to_lowercase();
    let ref_lower = reference.to_lowercase();
    let ref_words: Vec<String> = ref_lower
        .split_whitespace()
        .filter(|w| w.chars().count() > 2)
        .map(|w| w.to_string())
        .collect();
    if ref_words.is_empty() {
        // Short reference text — fall back to character-level overlap.
        // Check if >40% of reference chars appear in candidate.
        let ref_lower = reference.to_lowercase();
        let cand_lower = candidate.to_lowercase();
        let shared = ref_lower
            .chars()
            .filter(|c| !c.is_whitespace())
            .filter(|c| cand_lower.contains(*c))
            .count();
        let total = ref_lower.chars().filter(|c| !c.is_whitespace()).count();
        return total > 0 && shared * 100 / total > 40;
    }
    for cw in cand_lower.split_whitespace() {
        if cw.chars().count() <= 2 {
            continue;
        }
        for rw in &ref_words {
            if cw == *rw || rw.contains(&*cw) || cw.contains(&**rw) {
                return true;
            }
        }
    }
    false
}

fn common_prefix_bytes(a: &str, b: &str) -> usize {
    let mut bytes = 0;
    for (ac, bc) in a.chars().zip(b.chars()) {
        if ac != bc {
            break;
        }
        bytes += ac.len_utf8();
    }
    bytes
}

fn common_suffix_bytes(a: &str, b: &str) -> usize {
    let mut bytes = 0;
    for (ac, bc) in a.chars().rev().zip(b.chars().rev()) {
        if ac != bc {
            break;
        }
        bytes += ac.len_utf8();
    }
    bytes
}

/// Returns true only if `user_kept` is *meaningfully* different from `polished`.
///
/// Filters out false positives caused by:
/// - Whitespace-only changes (trailing newline, extra space)
/// - Case-only changes (auto-capitalize)
/// - Smart-punctuation substitutions (smart quotes, em-dashes, ellipsis)
/// - AX read jitter (< 3 character differences) — **except** when a jargon-
///   like token (digits + letters mixed, e.g. n8n, k8s, v2.0) is involved,
///   which is exactly the case where small char diffs ARE meaningful.
fn is_meaningful_edit(polished: &str, user_kept: &str) -> bool {
    let p_raw = normalize_spacing_and_punctuation(polished);
    let k_raw = normalize_spacing_and_punctuation(user_kept);
    let p = p_raw.to_lowercase();
    let k = k_raw.to_lowercase();

    if p == k {
        tracing::info!("[edit-gate] normalized texts identical — not meaningful");
        return false;
    }

    // Word-level check: at least 1 alphanumeric word must actually differ.
    // Compute this first so the char-distance gate can be context-aware.
    let p_words: Vec<&str> = p.split_whitespace().collect();
    let k_words: Vec<&str> = k.split_whitespace().collect();
    let p_raw_words: Vec<&str> = p_raw.split_whitespace().collect();
    let k_raw_words: Vec<&str> = k_raw.split_whitespace().collect();
    let max_len = p_words.len().max(k_words.len());
    let mut word_diffs = 0usize;
    let mut jargon_diff = false;
    for i in 0..max_len {
        let pw = p_words.get(i).copied().unwrap_or("");
        let kw = k_words.get(i).copied().unwrap_or("");
        let pw_raw = p_raw_words.get(i).copied().unwrap_or("");
        let kw_raw = k_raw_words.get(i).copied().unwrap_or("");
        let pw_core = alnum_word_core(pw);
        let kw_core = alnum_word_core(kw);
        if pw_core != kw_core && (!pw_core.is_empty() || !kw_core.is_empty()) {
            word_diffs += 1;
            // Jargon signal: if EITHER side of the diff has digits, the edit
            // is almost certainly a meaningful jargon correction (n8n, k8s,
            // v2.0, IP0 → IPO, etc.) regardless of how few chars differ.
            if looks_jargon_like_word(pw_raw) || looks_jargon_like_word(kw_raw) {
                jargon_diff = true;
            }
        }
    }

    if word_diffs == 0 {
        tracing::info!(
            "[edit-gate] no alphanumeric word diffs — punctuation/formatting only, not meaningful"
        );
        return false;
    }

    // Character-level distance gate.  Threshold = 1 for jargon edits (any
    // diff matters), 3 for plain prose (filter AX jitter / autocorrect).
    let char_diff = simple_char_distance(&p, &k);
    let min_char_diff = if jargon_diff { 1 } else { 3 };
    if char_diff < min_char_diff {
        tracing::info!(
            "[edit-gate] char distance {char_diff} < {min_char_diff} — AX jitter, not meaningful"
        );
        return false;
    }

    tracing::info!(
        "[edit-gate] {word_diffs} word(s) changed, char_diff={char_diff}, jargon={jargon_diff} — meaningful edit"
    );
    true
}

/// Normalize text for edit comparison: collapse whitespace and replace common
/// Unicode punctuation variants with ASCII equivalents while preserving case.
fn normalize_spacing_and_punctuation(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('\u{201c}', "\"") // left double smart quote
        .replace('\u{201d}', "\"") // right double smart quote
        .replace('\u{2018}', "'") // left single smart quote
        .replace('\u{2019}', "'") // right single smart quote / apostrophe
        .replace('\u{2014}', "-") // em-dash
        .replace('\u{2013}', "-") // en-dash
        .replace('\u{2026}', "...") // ellipsis
        .replace('\u{00a0}', " ") // non-breaking space
}

fn looks_jargon_like_word(word: &str) -> bool {
    let trimmed =
        word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '-' && c != '.');
    if trimmed.is_empty() {
        return false;
    }

    let has_digit = trimmed.chars().any(|c| c.is_ascii_digit());
    let alpha_len = trimmed.chars().filter(|c| c.is_ascii_alphabetic()).count();
    let all_caps_alpha = alpha_len >= 2
        && alpha_len <= 8
        && trimmed
            .chars()
            .all(|c| !c.is_ascii_lowercase() || !c.is_ascii_alphabetic())
        && trimmed.chars().any(|c| c.is_ascii_uppercase());
    let has_upper = trimmed.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = trimmed.chars().any(|c| c.is_ascii_lowercase());
    let mixed_case = has_upper && has_lower;
    let codey_punct = trimmed.contains('_') || trimmed.contains('-') || trimmed.contains('.');

    has_digit || all_caps_alpha || mixed_case || codey_punct
}

fn alnum_word_core(word: &str) -> &str {
    let start = word
        .find(|c: char| c.is_alphanumeric())
        .unwrap_or(word.len());
    let end = word
        .char_indices()
        .rfind(|(_, c)| c.is_alphanumeric())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(start);
    &word[start..end]
}

/// Simple positional character distance (diff chars at same index + length diff).
fn simple_char_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let min_len = a_chars.len().min(b_chars.len());
    let mut diff = a_chars.len().abs_diff(b_chars.len());
    for i in 0..min_len {
        if a_chars[i] != b_chars[i] {
            diff += 1;
        }
    }
    diff
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

fn parse_enterprise_oauth_token(args: &[String]) -> Option<String> {
    for arg in args {
        let trimmed = arg.trim();
        if !trimmed.starts_with("airnote://auth/callback") {
            continue;
        }
        let query = trimmed.split('?').nth(1)?;
        for part in query.split('&') {
            if let Some(token) = part.strip_prefix("token=") {
                let token = token.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
        }
    }
    None
}

fn emit_enterprise_oauth_token(app: &tauri::AppHandle, token: &str) {
    enterprise_oauth::emit_token(app, token);
}

fn handle_enterprise_oauth_urls(app: &tauri::AppHandle, urls: &[String]) {
    if let Some(token) = parse_enterprise_oauth_token(urls) {
        emit_enterprise_oauth_token(app, &token);
    }
}

fn main() {
    configure_ggml_metal_shutdown_guard();
    install_rustls_crypto_provider();

    // 1. Load env vars from .env files
    said_core::load_env();

    const DEFAULT_DIAGNOSTICS_BASE: &str = "https://airnote.emiactech.com";
    let diagnostics_base = std::env::var("AIRNOTE_DIAGNOSTICS_URL")
        .or_else(|_| std::env::var("AIRNOTE_CONTROL_PLANE_URL"))
        .unwrap_or_else(|_| DEFAULT_DIAGNOSTICS_BASE.to_string());
    said_core::reporter::configure(&diagnostics_base);
    said_core::reporter::set_phase("starting");

    // 1a. Sentry telemetry — must init before tracing so its panic hook
    //     stacks correctly. Held until main returns.
    let _sentry_guard = said_core::telemetry::init("said-desktop");

    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let summary = info.to_string();
        // A guarded main-thread callback will catch this panic and the app
        // survives. Report it as recovered and return — do not run the default
        // (abort-printing) hook.
        if in_guarded_section() {
            said_core::reporter::report_event(
                "panic.recovered",
                said_core::reporter::Severity::Error,
                serde_json::json!({
                    "summary": summary.chars().take(500).collect::<String>(),
                }),
            );
            tracing::error!(
                "[panic] caught in guarded callback — recovering: {}",
                summary.chars().take(300).collect::<String>()
            );
            return;
        }
        // Unguarded panic. This is NOT necessarily fatal: tokio catches panics in
        // spawned tasks, so the process usually survives and the state watchdog
        // heals any stuck state. Therefore we must NOT kill the backend here —
        // doing so would take the sidecar down on a fully recoverable task panic.
        // A genuinely fatal abort is cleaned up by `reap_previous()` on next launch.
        said_core::reporter::report_event(
            "panic",
            said_core::reporter::Severity::Fatal,
            serde_json::json!({
                "summary": summary.chars().take(500).collect::<String>(),
            }),
        );
        default_panic_hook(info);
    }));

    // 2. Tracing — platform-appropriate log dir; survives in bundled app
    let log_dir = said_core::paths::log_dir();
    std::fs::create_dir_all(&log_dir).ok();
    let log_path = log_dir.join("said.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| {
            eprintln!(
                "[main] cannot open log file {}: {e}; continuing with stderr logging only",
                log_path.display()
            );
            e
        })
        .ok();
    // Tracing layers: log file when writable, stderr (for `cargo run`
    // visibility), Sentry, and diagnostics reporter.
    {
        use tracing_subscriber::fmt;
        use tracing_subscriber::prelude::*;

        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,said_hotkey=debug,said_paster=debug".into());

        if let Some(log_file) = log_file {
            let file_layer = fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(log_file));
            let stderr_layer = fmt::layer().with_ansi(true).with_writer(std::io::stderr);
            tracing_subscriber::registry()
                .with(filter)
                .with(file_layer)
                .with(stderr_layer)
                .with(said_core::telemetry::tracing_layer())
                .with(said_core::reporter::tracing_layer())
                .init();
        } else {
            let stderr_layer = fmt::layer().with_ansi(true).with_writer(std::io::stderr);
            tracing_subscriber::registry()
                .with(filter)
                .with(stderr_layer)
                .with(said_core::telemetry::tracing_layer())
                .with(said_core::reporter::tracing_layer())
                .init();
        }
    }
    tracing::info!(
        "[main] said desktop starting — log file: {}",
        log_path.display()
    );
    if let Err(error) = stt_policy::normalize_persisted_prefs() {
        tracing::warn!("[stt-policy] startup preference normalization failed: {error}");
    }

    // 3. Shared state
    let shared_app = Arc::new(Mutex::new(DesktopApp::new()));
    let backend_arc = Arc::new(Mutex::new(None::<BackendEndpoint>));

    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    let builder = builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![AUTOSTART_ARG]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Only one AirNote runs at a time (keyed by bundle id). A second
            // launch — e.g. opening /Applications/AirNote.app while a copy is
            // already running — forwards here instead of starting a new process.
            // Bring the existing instance to the front so the relaunch is not a
            // silent no-op (the running instance keeps the single Dock icon).
            show_main_window(app);
            handle_enterprise_oauth_urls(app, &argv);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup({
            let shared = Arc::clone(&shared_app);
            move |app| {
                #[cfg(target_os = "macos")]
                {
                    // Keep the app foreground/regular so the Dock icon remains
                    // visible. The recorder pill is a separate non-activating
                    // NSPanel, so we no longer switch the whole app to Accessory.
                    app.set_activation_policy(tauri::ActivationPolicy::Regular);
                    tracing::info!("[main] macOS activation policy set to Regular (dock visible)");
                    if let Some(w) = app.get_webview_window("main") {
                        configure_main_window_macos(&w);
                    }
                }

                // Crash recovery: repair WAV headers for any meeting interrupted
                // mid-recording/processing on the previous run (so its audio is
                // playable), then re-enqueue any meeting that never finished
                // transcribing so the pipeline self-heals. Runs off-thread to keep
                // startup responsive; best-effort and self-contained.
                {
                    let recovery_handle = app.handle().clone();
                    std::thread::Builder::new()
                        .name("meeting-recovery".to_string())
                        .spawn(move || {
                            meeting_engine::recover_incomplete_meetings();
                            // Reclaim empty orphan dirs from past sessions that
                            // captured nothing (invisible in the UI otherwise).
                            meeting_engine::gc_orphan_meeting_dirs();
                            // Install the bundled Silero VAD into the data dir so
                            // dictation gets the VAD gate offline on first run.
                            meeting_engine::ensure_bundled_silero_vad();
                            // Ensure a model is selected if any is installed.
                            meeting_engine::ensure_active_model_sync();
                            // Warm this platform's dictation provider so the
                            // first real utterance avoids setup costs (macOS:
                            // model load; Windows: key check + HTTP client).
                            dictation_stt::prewarm();
                            recovery_handle
                                .state::<meeting_engine::MeetingEngineState>()
                                .requeue_interrupted_meetings();
                        })
                        .ok();
                }

                let desktop_prefs = said_core::prefs::load();
                if let Err(e) = apply_launch_at_login(app.handle(), desktop_prefs.launch_at_login)
                {
                    tracing::warn!("[prefs] launch_at_login sync failed: {e}");
                }

                {
                    use tauri_plugin_deep_link::DeepLinkExt;
                    let handle = app.handle().clone();
                    app.deep_link().on_open_url(move |event| {
                        let urls: Vec<String> =
                            event.urls().iter().map(|u| u.to_string()).collect();
                        handle_enterprise_oauth_urls(&handle, &urls);
                    });

                    if let Ok(Some(urls)) = app.deep_link().get_current() {
                        let url_strings: Vec<String> =
                            urls.iter().map(|u| u.to_string()).collect();
                        if let Some(token) = parse_enterprise_oauth_token(&url_strings) {
                            let handle = app.handle().clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(600));
                                emit_enterprise_oauth_token(&handle, &token);
                            });
                        }
                    } else if let Some(token) =
                        parse_enterprise_oauth_token(&std::env::args().collect::<Vec<_>>())
                    {
                        let handle = app.handle().clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(600));
                            emit_enterprise_oauth_token(&handle, &token);
                        });
                    }

                    #[cfg(any(windows, target_os = "linux"))]
                    {
                        if let Err(e) = app.deep_link().register_all() {
                            tracing::warn!("[deep-link] register_all failed: {e}");
                        }
                    }
                }

                // ── Request notification permission (macOS) ─────────────────
                #[cfg(target_os = "macos")]
                {
                    use tauri_plugin_notification::NotificationExt;
                    match app.notification().request_permission() {
                        Ok(perm) => tracing::info!("[perm] Notifications={perm:?}"),
                        Err(e) => tracing::warn!("[perm] Notification permission request failed: {e}"),
                    }
                }

                // ── Spawn backend daemon ──────────────────────────────────────
                // ── Permission status at launch (visible in the said.log file under the platform log dir) ──
                let ax_ok = paster::is_accessibility_granted();
                let im_ok = hotkey::is_input_monitoring_granted();
                tracing::info!("[perm] Accessibility={ax_ok} InputMonitoring={im_ok}");
                if !ax_ok {
                    tracing::warn!("[perm] Accessibility NOT granted — paste will fail. Grant in System Settings → Privacy → Accessibility");
                }
                if !im_ok {
                    #[cfg(target_os = "macos")]
                    tracing::warn!(
                        "[perm] Input Monitoring NOT granted — hotkeys (Caps Lock, Option+1-5, Ctrl+Cmd+V) will not work. Grant in System Settings → Privacy → Input Monitoring"
                    );
                    #[cfg(not(target_os = "macos"))]
                    tracing::warn!(
                        "[perm] is_input_monitoring_granted() returned false on non-macOS — this should not happen (Windows always returns true). Hotkey may not work."
                    );
                }

                let using_external_backend = backend::external_backend_url().is_some();
                if using_external_backend {
                    tracing::info!("[main] SAID_EXTERNAL_BACKEND_URL set — skipping backend reap");
                } else {
                    backend_guard::reap_previous();
                }
                match backend::spawn() {
                    Ok(handle) => {
                        // Extract all endpoint clones BEFORE storing (move) the handle.
                        let ep  = handle.endpoint();
                        let ep_recovery = handle.endpoint();
                        let ep_notifications = handle.endpoint();
                        // Store the full handle so Drop kills the child on app exit.
                        // Without this the child outlives the app (zombie leak).
                        let _ = install_backend_handle(app.handle(), handle);
                        tracing::info!("[main] backend daemon ready");

                        start_runtime_notification_listener(
                            app.handle().clone(),
                            ep_notifications,
                        );

                        // ── Crash recovery ─────────────────────────────────────
                        // If a previous run died mid-dictation, its audio is still on
                        // disk. Re-transcribe it and hand the user their words back.
                        {
                            let app_rec = app.handle().clone();
                            tauri::async_runtime::spawn(async move {
                                let Some(wav) = recovery::take_orphan() else {
                                    return;
                                };
                                // Let the backend finish coming up before transcribing.
                                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                                diag::breadcrumb("recovery:attempt");
                                match recover_transcribe(&ep_recovery, wav).await {
                                    Ok(done) => {
                                        let text = if !done.polished.trim().is_empty() {
                                            done.polished
                                        } else {
                                            done.transcript
                                        };
                                        if text.trim().is_empty() {
                                            tracing::info!(
                                                "[recovery] orphan produced empty transcript — nothing to recover"
                                            );
                                            return;
                                        }
                                        tracing::warn!(
                                            "[recovery] recovered {} chars from a crashed dictation",
                                            text.len()
                                        );
                                        diag::breadcrumb("recovery:recovered");
                                        if let Ok(mut g) = app_rec.state::<LatestResult>().0.lock() {
                                            *g = Some(text.clone());
                                        }
                                        show_main_window(&app_rec);
                                        let _ = app_rec.emit(
                                            "dictation-recovered",
                                            serde_json::json!({ "text": text }),
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!("[recovery] re-transcribe failed: {e}");
                                    }
                                }
                            });
                        }
                        // Seed the tray cache with real prefs so the first tray
                        // menu already shows the correct model checkmark.
                        let app_h = app.handle().clone();
                        tauri::async_runtime::spawn(async move {
                            let prefs_res = api::get_preferences(&ep).await;
                            if let Ok(prefs) = &prefs_res {
                                #[cfg(any(target_os = "macos", target_os = "windows"))]
                                hotkey::set_record_hotkey(parse_record_hotkey(&prefs.record_hotkey));
                                if let Ok(mut cache) = app_h.state::<TrayCache>().0.lock() {
                                    cache.custom_prompt   = prefs.custom_prompt.clone();
                                    cache.output_language = prefs.output_language.clone();
                                    cache.record_hotkey   = prefs.record_hotkey.clone();
                                }
                                // Re-render now that we have real data
                                let shared = app_h.state::<SharedApp>();
                                if let Ok(d) = shared.0.lock() {
                                    let snap = d.snapshot();
                                    drop(d);
                                    sync_tray(&app_h, &snap);
                                }
                            }
                            // Seed hot-path cache so the first recording needs zero HTTP.
                            let language = prefs_res
                                .as_ref()
                                .ok()
                                .map(|p| p.language.clone())
                                .unwrap_or_default();
                            // Seed meeting AI's Groq key from Settings → API keys.
                            meeting_engine::set_runtime_groq_api_key(
                                prefs_res.as_ref().ok().and_then(|p| p.groq_api_key.clone()),
                            );
                            let hot = app_h.state::<HotPathCache>();
                            let mut c = hot.0.write().await;
                            c.language = language;
                        });
                    }
                    Err(e) => {
                        tracing::error!("[main] failed to spawn backend: {e}");
                        said_core::reporter::report_event(
                            "backend.spawn_failed",
                            said_core::reporter::Severity::Error,
                            serde_json::json!({ "error": e.to_string() }),
                        );
                        // App continues without backend; commands return errors.
                    }
                }
                start_backend_watchdog(app.handle().clone(), using_external_backend);

                // signal_hook's iterator module is `cfg(not(windows))` upstream —
                // POSIX signals don't exist on Windows the same way (Ctrl+C is
                // handled via SetConsoleCtrlHandler / WM_CLOSE). For v3.0 we
                // skip the SIGTERM/SIGINT thread on Windows; the Tauri
                // RunEvent::ExitRequested handler still cleans up the backend.
                #[cfg(not(windows))]
                {
                    let app_handle = app.handle().clone();
                    let cleanup_owned_backend = !using_external_backend;
                    std::thread::spawn(move || {
                        let signals = signal_hook::iterator::Signals::new([
                            signal_hook::consts::SIGINT,
                            signal_hook::consts::SIGTERM,
                        ]);
                        let Ok(mut signals) = signals else {
                            tracing::warn!("[main] failed to install signal hook");
                            return;
                        };
                        if signals.forever().next().is_some() {
                            if cleanup_owned_backend {
                                backend_guard::kill_from_pid_file();
                            }
                            app_handle.exit(0);
                        }
                    });
                }

                // ── System tray ───────────────────────────────────────────────
                // Build the initial menu from a fresh snapshot. It will be
                // rebuilt by `sync_tray()` on every state change.
                let initial_snap = shared.lock().ok().map(|d| d.snapshot());
                // Initial menu uses defaults (model=smart, no custom prompt) —
                // sync_tray() will refresh it with real prefs once the backend is ready.
                let initial_menu = match &initial_snap {
                    Some(snap) => build_tray_menu(app.handle(), snap, None, "hinglish")?,
                    None => Menu::with_items(app, &[
                        &MenuItem::with_id(app, "show", "Open AirNote", true, None::<&str>)?,
                        &PredefinedMenuItem::separator(app)?,
                        &MenuItem::with_id(app, "quit", "Quit AirNote", true, None::<&str>)?,
                    ])?,
                };

                // macOS uses a monochrome template glyph (auto-tinted by the OS);
                // on Windows that white-on-transparent glyph is near-invisible in
                // the tray, so ship the full-color app icon there.
                #[cfg(target_os = "macos")]
                let tray_icon = tauri::image::Image::from_bytes(
                    include_bytes!("../icons/tray@2x.png")
                ).ok();
                #[cfg(not(target_os = "macos"))]
                let tray_icon = tauri::image::Image::from_bytes(
                    include_bytes!("../icons/32x32.png")
                ).ok();

                let mut tray_builder = TrayIconBuilder::with_id("said")
                    .tooltip("AirNote — Voice Polish Studio")
                    .menu(&initial_menu)
                    .show_menu_on_left_click(true);

                if let Some(icon) = tray_icon {
                    tray_builder = tray_builder.icon(icon);
                    // Template = monochrome auto-tint; macOS only.
                    #[cfg(target_os = "macos")]
                    {
                        tray_builder = tray_builder.icon_as_template(true);
                    }
                }

                tray_builder
                    .on_menu_event(|app, event| {
                        // Seatbelt: a panic in any handler here runs inside an AppKit
                        // callback and would SIGABRT the whole app — catch + recover.
                        guard_panics("tray.menu_event", || {
                            let id = event.id.as_ref();
                            match id {
                                "tray_toggle" => tray_toggle_recording(app),
                                "show" => show_main_window(app),
                                "settings"  => tray_open_settings(app),
                                "quit"      => app.exit(0),
                                // Output language switch
                                _ if id.starts_with("tray_lang_") => {
                                    let lang = &id["tray_lang_".len()..];
                                    tray_set_output_language(app, lang);
                                }
                                "tray_smart_repair" => smart_repair_last(app),
                                // Polish my message — tone preset suffix
                                _ if id.starts_with("tray_polish_") => {
                                    let tone = &id["tray_polish_".len()..];
                                    tray_polish_message(app, tone);
                                }
                                _ => tracing::warn!("[tray] unhandled menu id={id}"),
                            }
                        });
                    })
                    .build(app)?;

                // ── Close window → hide (keep running in menu bar) ────────────
                if let Some(window) = app.get_webview_window("main") {
                    let win = window.clone();
                    window.on_window_event(move |event| {
                        guard_panics("window.main_event", || {
                            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                                api.prevent_close();
                                // macOS can leave an empty black fullscreen Space if a
                                // transparent/hidden webview is hidden directly from
                                // fullscreen. First escape fullscreen; a later close can
                                // hide the normal window.
                                if win.is_fullscreen().unwrap_or(false) {
                                    let _ = win.set_fullscreen(false);
                                    let _ = win.show();
                                    let _ = win.unminimize();
                                    let _ = win.set_focus();
                                    return;
                                }
                                let _ = win.hide();
                            }
                        });
                    });
                }

                // ── HUD surface: status-bar pill by default; native notch opt-in ─
                // Keep the notch sidecar code staged for later launch, but do not
                // connect it in normal builds. Explicitly opt in with
                // AIRNOTE_NOTCH_SIDECAR=1 while developing the notch surface.
                {
                    let notch_enabled = std::env::var("AIRNOTE_NOTCH_SIDECAR")
                        .map(|v| v == "1")
                        .unwrap_or(false);
                    let mut notch_active = false;
                    if notch_enabled {
                        let action_handle = app.handle().clone();
                        if let Some(sidecar) = notch_sidecar::spawn(move |action| {
                            handle_notch_action(&action_handle, action)
                        }) {
                            wire_notch_events(app.handle(), &sidecar);
                            app.manage(sidecar);
                            NOTCH_FIRST_CLASS.store(true, Ordering::SeqCst);
                            notch_active = true;
                            tracing::info!(
                                "[notch] sidecar opt-in active — status-bar pill suppressed"
                            );
                        } else {
                            tracing::warn!("[notch] sidecar binary not found — using status-bar pill");
                        }
                    }
                    if !notch_active {
                        tracing::info!(
                            "[status-bar] startup preload scheduled — HUD will be created hidden after ready"
                        );
                    }
                }

                // ── Frontend-ready handshake ───────────────────────────────────
                // The status-bar WebView emits "frontend-ready" once all its event
                // listeners are registered. We re-sync immediately so any state that
                // was set before the listeners were up gets delivered.
                {
                    use tauri::Listener as _;
                    let app_fh = app.handle().clone();
                    app.listen("frontend-ready", move |_| {
                        tracing::debug!("[status-bar] frontend-ready received — resyncing state");
                        emit_status_bar_resync(&app_fh, "frontend-ready");
                    });
                }

                // ── Backend-authoritative "End meeting" (redundant backstop) ───
                // LiveMeetingView also invokes stop_session directly; this event
                // listener is a belt-and-suspenders path that stops the meeting
                // without a per-call JS callback. stop() can block (worker-thread
                // joins up to a timeout), so run it on the blocking pool.
                {
                    use tauri::Listener as _;
                    let app_stop = app.handle().clone();
                    app.listen("meeting/request-stop", move |_| {
                        let app = app_stop.clone();
                        tauri::async_runtime::spawn_blocking(move || {
                            let state = app.state::<meeting_engine::MeetingEngineState>();
                            let _ = meeting_engine::request_stop(&app, state.inner());
                        });
                    });
                }

                // ── Pre-create the floating meeting pill (Windows/Linux) ───────
                // The pill is created ONCE here, hidden, at startup — then only
                // shown/hidden during meetings. Creating a WebView2 window from
                // inside the focus-change IPC command (the old behavior) deadlocks
                // Windows IPC (wry #583) and wedged End-meeting; creating during
                // setup — never inside an in-flight invoke — is safe. macOS builds
                // its NSPanel lazily and is unaffected.
                #[cfg(not(target_os = "macos"))]
                {
                    let h = app.handle().clone();
                    if h.get_webview_window("meeting-pill").is_none() {
                        match tauri::WebviewWindowBuilder::new(
                            &h,
                            "meeting-pill",
                            tauri::WebviewUrl::App("index.html?view=meeting-pill#meeting-pill".into()),
                        )
                        .title("AirNote Meeting")
                        .inner_size(MEETING_PILL_W, MEETING_PILL_H)
                        .decorations(false)
                        .always_on_top(true)
                        .visible_on_all_workspaces(true)
                        .skip_taskbar(true)
                        .focused(false)
                        .resizable(false)
                        .shadow(false)
                        .transparent(true)
                        .visible(false)
                        .build()
                        {
                            Ok(_) => tracing::info!("[meeting-pill] pre-created (hidden)"),
                            Err(e) => tracing::warn!("[meeting-pill] pre-create failed: {e}"),
                        }
                    }
                }

                // ── State self-heal watchdog ───────────────────────────────────
                // If the finish pipeline dies (caught panic, killed task, lost
                // event) the state machine can wedge in "processing", which blocks
                // every new recording. A legitimate polish takes seconds; detect a
                // processing state that persists far beyond that and reset it so
                // dictation keeps working — and recover the captured audio.
                {
                    let app_sw = app.handle().clone();
                    let shared_sw = Arc::clone(&shared);
                    tauri::async_runtime::spawn(async move {
                        // Threshold beyond which a "processing" state is considered
                        // wedged. A real polish takes seconds; 60s default never
                        // false-triggers. Lowered via env for fast soak testing.
                        let stuck_secs: u64 = std::env::var("AIRNOTE_HEAL_STUCK_SECS")
                            .ok()
                            .and_then(|v| v.parse().ok())
                            .filter(|&n| n >= 5)
                            .unwrap_or(60);
                        let stuck_recording_secs: u64 =
                            std::env::var("AIRNOTE_HEAL_RECORDING_SECS")
                                .ok()
                                .and_then(|v| v.parse().ok())
                                .filter(|&n| n >= 30)
                                .unwrap_or(180);
                        let tick_secs = (stuck_secs / 4).clamp(2, 15);
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(tick_secs));
                        interval.tick().await; // skip the immediate first tick
                        let mut processing_secs: u64 = 0;
                        let mut recording_secs: u64 = 0;
                        loop {
                            interval.tick().await;
                            let state = shared_sw
                                .lock()
                                .map(|d| d.state)
                                .unwrap_or_else(|p| p.into_inner().state);
                            if state == desktop::AppState::Processing {
                                processing_secs += tick_secs;
                                recording_secs = 0;
                                if processing_secs >= stuck_secs {
                                    tracing::warn!(
                                        "[heal] processing state stuck ~{processing_secs}s — healing"
                                    );
                                    heal_stuck_state(&app_sw, "watchdog_stuck_processing");
                                    processing_secs = 0;
                                }
                            } else if state == desktop::AppState::Recording {
                                processing_secs = 0;
                                recording_secs += tick_secs;
                                let long_locked = app_sw
                                    .try_state::<LongDictationState>()
                                    .map(|s| s.locked.load(Ordering::SeqCst))
                                    .unwrap_or(false);
                                if !long_locked && recording_secs >= stuck_recording_secs {
                                    tracing::error!(
                                        "[heal] recording state stuck ~{recording_secs}s — auto-finishing"
                                    );
                                    diag::breadcrumb("heal:stuck_recording_finish");
                                    said_core::reporter::report_event(
                                        "state.recording_stuck_auto_finish",
                                        said_core::reporter::Severity::Error,
                                        serde_json::json!({
                                            "recording_secs": recording_secs,
                                            "trail": diag::breadcrumbs(20),
                                        }),
                                    );
                                    let shared_finish = Arc::clone(&shared_sw);
                                    let app_finish = app_sw.clone();
                                    let backend_finish =
                                        Arc::clone(&app_sw.state::<BackendState>().0);
                                    std::thread::spawn(move || {
                                        do_finish_recording(
                                            shared_finish,
                                            app_finish,
                                            backend_finish,
                                        );
                                    });
                                    recording_secs = 0;
                                }
                            } else {
                                processing_secs = 0;
                                recording_secs = 0;
                            }
                        }
                    });
                }

                // ── Chaos soak (env-gated, inert in production) ────────────────
                // With AIRNOTE_CHAOS=1 AIRNOTE_CHAOS_SOAK=1 the app self-injects
                // faults on a loop so a monitor can verify it tortures-and-heals.
                chaos::maybe_start_soak(app.handle());

                // ── HUD visibility watchdog (macOS) ────────────────────────────
                // Keep this watchdog narrow and deterministic: it only checks
                // whether the HUD disappeared while the app is active, and then
                // re-presents it on demand. Avoid periodic panel retuning here;
                // repeated AppKit mutation while idle is the crash suspect.
                #[cfg(target_os = "macos")]
                {
                    let app_wdg = app.handle().clone();
                    let shared_wdg = Arc::clone(&shared);
                    tauri::async_runtime::spawn(async move {
                        let mut interval =
                            tokio::time::interval(std::time::Duration::from_secs(30));
                        interval.tick().await; // skip the immediate first tick
                        loop {
                            interval.tick().await;
                            let is_active = shared_wdg
                                .lock()
                                .ok()
                                .map(|d| d.state != desktop::AppState::Idle)
                                .unwrap_or(false);
                            if !is_active {
                                continue;
                            }

                            let visible = app_wdg
                                .get_webview_window("status-bar")
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(true);
                            if !visible {
                                tracing::warn!(
                                    "[hud-watchdog] HUD hidden while state is active — recovering"
                                );
                                diag::breadcrumb("hud_watchdog:recover");
                                let _ = present_status_bar_native(
                                    &app_wdg,
                                    "watchdog-recover",
                                    true,
                                );
                            }
                        }
                    });
                }

                // ── Hold-to-record hotkey ─────────────────────────────────────
                // macOS: CGEventTap (see said_hotkey::imp). Windows: WH_KEYBOARD_LL
                // (see said_hotkey::imp_windows). Linux: no-op stub.
                //
                // In meeting capture, Fn/Caps acts as a quick mute. Once muted,
                // the hotkey goes back to normal AirNote dictation until the user
                // resumes meeting capture from the dock control.
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                {
                    let shared_hold_press = Arc::clone(&app.state::<SharedApp>().0);
                    let shared_hold_release = Arc::clone(&app.state::<SharedApp>().0);
                    let back_hold_press = Arc::clone(&app.state::<BackendState>().0);
                    let back_hold_release = Arc::clone(&app.state::<BackendState>().0);
                    let long_locked_press = Arc::clone(&app.state::<LongDictationState>().locked);
                    let long_locked_release =
                        Arc::clone(&app.state::<LongDictationState>().locked);
                    let long_stop_consumed_press =
                        Arc::clone(&app.state::<LongDictationState>().stop_consumed);
                    let long_stop_consumed_release =
                        Arc::clone(&app.state::<LongDictationState>().stop_consumed);
                    let long_finishing_press =
                        Arc::clone(&app.state::<LongDictationState>().finishing);
                    let meeting_active_press = Arc::clone(&app.state::<MeetingModeState>().active);
                    let meeting_muted_press = Arc::clone(&app.state::<MeetingModeState>().muted);
                    let meeting_generation_press =
                        Arc::clone(&app.state::<MeetingModeState>().generation);
                    let hotkey_meeting_mute = Arc::new(AtomicBool::new(false));
                    let hotkey_meeting_mute_release = Arc::clone(&hotkey_meeting_mute);
                    let app_press = app.handle().clone();
                    let app_release = app.handle().clone();
                    hotkey::start_hold_listener(
                        Arc::new(move || {
                            let meeting_capture = meeting_active_press.load(Ordering::SeqCst)
                                && !meeting_muted_press.load(Ordering::SeqCst);
                            let shared = Arc::clone(&shared_hold_press);
                            let app_h = app_press.clone();
                            if meeting_capture {
                                hotkey_meeting_mute.store(true, Ordering::SeqCst);
                                meeting_muted_press.store(true, Ordering::SeqCst);
                                meeting_generation_press.fetch_add(1, Ordering::SeqCst);
                                emit_meeting_stt_status(&app_h);
                                std::thread::spawn(move || {
                                    let current = hotkey_current_state(&shared, "mute");
                                    if current == Some(desktop::AppState::Recording) {
                                        do_cancel_recording(shared, app_h, "hotkey mute");
                                    }
                                });
                            } else {
                                let back = Arc::clone(&back_hold_press);
                                let long_locked = Arc::clone(&long_locked_press);
                                let long_stop_consumed = Arc::clone(&long_stop_consumed_press);
                                let long_finishing = Arc::clone(&long_finishing_press);
                                HOTKEY_START_IN_FLIGHT.store(true, Ordering::SeqCst);
                                std::thread::spawn(move || {
                                    struct HotkeyStartGuard;
                                    impl Drop for HotkeyStartGuard {
                                        fn drop(&mut self) {
                                            HOTKEY_START_IN_FLIGHT.store(false, Ordering::SeqCst);
                                        }
                                    }
                                    let _guard = HotkeyStartGuard;
                                    let current = hotkey_current_state(&shared, "start");
                                    if long_locked.load(Ordering::SeqCst)
                                        && current == Some(desktop::AppState::Recording)
                                    {
                                        tracing::info!(
                                            "[hotkey] record key pressed while long dictation locked → process"
                                        );
                                        stop_long_dictation_lock(
                                            &app_h,
                                            &long_locked,
                                            &long_stop_consumed,
                                            &long_finishing,
                                        );
                                        do_finish_recording(shared, app_h, back);
                                    } else if current == Some(desktop::AppState::Idle) {
                                        do_start_recording(&shared, &app_h);
                                        if FINISH_AFTER_START.load(Ordering::SeqCst) {
                                            request_queued_finish(
                                                shared,
                                                app_h,
                                                back,
                                                "release_during_start",
                                            );
                                        }
                                    } else if current == Some(desktop::AppState::Processing) {
                                        if long_stop_consumed.load(Ordering::SeqCst)
                                            || long_finishing.load(Ordering::SeqCst)
                                        {
                                            tracing::debug!(
                                                "[hotkey] record press ignored while long dictation is finishing"
                                            );
                                            return;
                                        }
                                        // Pressing record again while the previous take is
                                        // still processing means the user abandoned it (e.g.
                                        // released the hotkey early by mistake). Reset that
                                        // run to idle and start a fresh recording immediately
                                        // — the generation bump in do_start_recording makes
                                        // the abandoned run skip its paste.
                                        tracing::info!(
                                            "[hotkey] record pressed during processing — abandoning previous take, starting fresh"
                                        );
                                        cancel_processing_run(&app_h, "record_pressed_during_processing");
                                        do_start_recording_without_cycle_gap(&shared, &app_h);
                                        if FINISH_AFTER_START.load(Ordering::SeqCst) {
                                            request_queued_finish(
                                                shared,
                                                app_h,
                                                back,
                                                "release_during_start",
                                            );
                                        }
                                    }
                                });
                            }
                        }),
                        Arc::new(move || {
                            let shared = Arc::clone(&shared_hold_release);
                            let app_h = app_release.clone();
                            if hotkey_meeting_mute_release.swap(false, Ordering::SeqCst) {
                                emit_meeting_stt_status(&app_h);
                            } else if long_stop_consumed_release.swap(false, Ordering::SeqCst) {
                                tracing::debug!("[hotkey] record key release consumed after long dictation stop");
                            } else if long_locked_release.load(Ordering::SeqCst) {
                                tracing::info!(
                                    "[hotkey] record key released while long dictation locked — keep listening"
                                );
                            } else {
                                // Normal AirNote route. This also applies while the
                                // meeting view is open but meeting capture is muted.
                                let back = Arc::clone(&back_hold_release);
                                schedule_release_mic_cleanup(
                                    Arc::clone(&shared),
                                    app_h.clone(),
                                    Arc::clone(&back),
                                    "record_hotkey_release",
                                );
                                std::thread::spawn(move || {
                                    let current = hotkey_current_state(&shared, "finish");
                                    if current == Some(desktop::AppState::Recording) {
                                        FINISH_AFTER_START.store(false, Ordering::SeqCst);
                                        do_finish_recording(shared, app_h, back);
                                    } else if (current == Some(desktop::AppState::Idle)
                                        || current.is_none())
                                        && (HOTKEY_START_IN_FLIGHT.load(Ordering::SeqCst)
                                            || RECORDING_STARTING.load(Ordering::SeqCst))
                                    {
                                        tracing::info!(
                                            "[hotkey] release arrived before recording started — queue finish"
                                        );
                                        request_queued_finish(
                                            shared,
                                            app_h,
                                            back,
                                            if current.is_none() {
                                                "release_lock_busy"
                                            } else {
                                                "release_before_start"
                                            },
                                        );
                                    } else if current.is_none() {
                                        tracing::info!(
                                            "[hotkey] release could not read state — queue guarded finish retry"
                                        );
                                        request_queued_finish(
                                            shared,
                                            app_h,
                                            back,
                                            "release_state_unknown",
                                        );
                                    }
                                });
                            }
                        }),
                    );

                    // ── Ctrl hold-to-talk → Divo (independent of the record hotkey) ──
                    {
                        let shared_dp = Arc::clone(&app.state::<SharedApp>().0);
                        let shared_dr = Arc::clone(&app.state::<SharedApp>().0);
                        let shared_dc = Arc::clone(&app.state::<SharedApp>().0);
                        let back_dp = Arc::clone(&app.state::<BackendState>().0);
                        let back_dr = Arc::clone(&app.state::<BackendState>().0);
                        let app_dp = app.handle().clone();
                        let app_dr = app.handle().clone();
                        let app_dc = app.handle().clone();
                        hotkey::register_divo_hotkey_callbacks(
                            // press → start a Divo-routed recording
                            Arc::new(move || {
                                let shared = Arc::clone(&shared_dp);
                                let app_h = app_dp.clone();
                                let back = Arc::clone(&back_dp);
                                HOTKEY_START_IN_FLIGHT.store(true, Ordering::SeqCst);
                                std::thread::spawn(move || {
                                    struct HotkeyStartGuard;
                                    impl Drop for HotkeyStartGuard {
                                        fn drop(&mut self) {
                                            HOTKEY_START_IN_FLIGHT.store(false, Ordering::SeqCst);
                                        }
                                    }
                                    // _guard lives outside guard_panics so the in-flight
                                    // flag is cleared on drop even if the body panics.
                                    let _guard = HotkeyStartGuard;
                                    guard_panics("divo.start", move || {
                                        let current = hotkey_current_state(&shared, "divo start");
                                        if current == Some(desktop::AppState::Idle) {
                                            DIVO_START_PENDING.store(true, Ordering::SeqCst);
                                            DIVO_FOLLOWUP_PENDING.store(false, Ordering::SeqCst);
                                            DIVO_NEW_CHAT_PENDING.store(false, Ordering::SeqCst);
                                            do_start_recording(&shared, &app_h);
                                            if FINISH_AFTER_START.load(Ordering::SeqCst) {
                                                request_queued_finish(
                                                    shared,
                                                    app_h,
                                                    back,
                                                    "divo_release_during_start",
                                                );
                                            }
                                        }
                                    });
                                });
                            }),
                            // release → finish & send to Divo
                            Arc::new(move || {
                                let shared = Arc::clone(&shared_dr);
                                let app_h = app_dr.clone();
                                let back = Arc::clone(&back_dr);
                                schedule_release_mic_cleanup(
                                    Arc::clone(&shared),
                                    app_h.clone(),
                                    Arc::clone(&back),
                                    "divo_hotkey_release",
                                );
                                std::thread::spawn(move || {
                                    guard_panics("divo.finish", move || {
                                        // Capture the Ctrl+N intent now, before the async
                                        // transcription, so the staged turn knows whether
                                        // to default to a new chat.
                                        DIVO_NEW_CHAT_PENDING
                                            .store(hotkey::divo_take_new_chat(), Ordering::SeqCst);
                                        let current = hotkey_current_state(&shared, "divo finish");
                                        if current == Some(desktop::AppState::Recording) {
                                            FINISH_AFTER_START.store(false, Ordering::SeqCst);
                                            do_finish_recording(shared, app_h, back);
                                        } else if (current == Some(desktop::AppState::Idle)
                                            || current.is_none())
                                            && (HOTKEY_START_IN_FLIGHT.load(Ordering::SeqCst)
                                                || RECORDING_STARTING.load(Ordering::SeqCst))
                                        {
                                            request_queued_finish(
                                                shared,
                                                app_h,
                                                back,
                                                "divo_release_before_start",
                                            );
                                        }
                                    });
                                });
                            }),
                            // cancel → a shortcut (Ctrl+C etc.) tainted the hold; drop it
                            Arc::new(move || {
                                let shared = Arc::clone(&shared_dc);
                                let app_h = app_dc.clone();
                                std::thread::spawn(move || {
                                    guard_panics("divo.cancel", move || {
                                        DIVO_START_PENDING.store(false, Ordering::SeqCst);
                                        DIVO_FOLLOWUP_PENDING.store(false, Ordering::SeqCst);
                                        DIVO_NEW_CHAT_PENDING.store(false, Ordering::SeqCst);
                                        let _ = hotkey::divo_take_new_chat();
                                        let current = hotkey_current_state(&shared, "divo cancel");
                                        if current == Some(desktop::AppState::Recording) {
                                            do_cancel_recording(shared, app_h, "divo ctrl shortcut");
                                        }
                                    });
                                });
                            }),
                        );
                        // Stays disabled until the webview pushes valid Divo credentials —
                        // except in local dev-direct mode, where we force it on so the
                        // feature can be exercised without a control-plane connection.
                        let divo_direct = std::env::var("AIRNOTE_DIVO_DIRECT")
                            .map(|s| !s.trim().is_empty())
                            .unwrap_or(false);
                        hotkey::set_divo_hotkey_enabled(divo_direct);
                        if divo_direct {
                            tracing::info!("[divo] dev-direct mode — Ctrl hotkey force-enabled");
                        }
                    }

                    let app_long = app.handle().clone();
                    let shared_long = Arc::clone(&app.state::<SharedApp>().0);
                    let long_pending = Arc::clone(&app.state::<LongDictationState>().pending_lock);
                    let meeting_active_long = Arc::clone(&app.state::<MeetingModeState>().active);
                    let meeting_muted_long = Arc::clone(&app.state::<MeetingModeState>().muted);
                    hotkey::register_long_dictation_callback(Arc::new(move || {
                        let app_h = app_long.clone();
                        let shared = Arc::clone(&shared_long);
                        let pending = Arc::clone(&long_pending);
                        let meeting_capture = meeting_active_long.load(Ordering::SeqCst)
                            && !meeting_muted_long.load(Ordering::SeqCst);
                        if meeting_capture {
                            tracing::info!(
                                "[hotkey] long dictation hotkey ignored — meeting capture owns the record key"
                            );
                            return;
                        }
                        std::thread::spawn(move || {
                            let current = hotkey_current_state(&shared, "long dictation");
                            if current == Some(desktop::AppState::Recording) {
                                activate_long_dictation_lock(&app_h);
                            } else if current == Some(desktop::AppState::Idle) {
                                pending.store(true, Ordering::SeqCst);
                                tracing::info!(
                                    "[hotkey] long dictation lock pending until recording starts"
                                );
                            }
                        });
                    }));

                    let app_hud = app.handle().clone();
                    hotkey::register_hud_shortcut_callback(Arc::new(move |action| {
                        let app_h = app_hud.clone();
                        std::thread::spawn(move || match action {
                            hotkey::HudShortcutAction::PlacementMode => {
                                toggle_status_bar_placement_mode(&app_h);
                            }
                            hotkey::HudShortcutAction::ResetPosition => {
                                tracing::info!("[hotkey] ⇧⌘. → reset status bar position");
                                reset_status_bar_to_default(&app_h);
                                show_status_bar_placement_mode(&app_h, "Centered");
                            }
                            hotkey::HudShortcutAction::ToggleMessagePolishMode => {
                                tracing::info!("[hotkey] ⇧⌘Space → toggle message polish mode");
                                toggle_message_polish_mode(&app_h);
                            }
                            hotkey::HudShortcutAction::RetryLastFromAudio => {
                                tracing::info!(
                                    "[hotkey] ⌃⌥R / Ctrl+Left-Alt+R → full retry of last dictation from saved audio"
                                );
                                retry_last_from_audio(&app_h);
                            }
                        });
                    }));

                    // ── Tone shortcuts ─────────────────────────────────────────
                    // Select text in any app, press Option+N on macOS or Alt+N on
                    // Windows to polish with a preset tone.
                    //
                    // IMPORTANT: the callback runs on the CGEventTap's CFRunLoop thread.
                    // We MUST NOT call read_selected_text() on that thread — its Cmd+C
                    // fallback posts synthetic key events that queue behind the running
                    // callback and never reach the target app.  Spawning a new thread
                    // lets the tap callback return immediately so the run-loop is unblocked.
                    let app_shortcut = app.handle().clone();
                    hotkey::register_shortcut_callback(Arc::new(move |n: u8| {
                        let app_clone = app_shortcut.clone();
                        std::thread::spawn(move || {
                            // Small delay to let the tap callback return and the
                            // CFRunLoop process queued events before we try Cmd+C.
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            match n {
                                1 => tray_polish_message(&app_clone, "message_polish"),
                                2 => tray_polish_message(&app_clone, "professional"),
                                3 => tray_polish_message(&app_clone, "casual"),
                                4 => tray_polish_message(&app_clone, "concise"),
                                5 => tray_polish_message(&app_clone, "hinglish"),
                                _ => {}
                            }
                        });
                    }));

                    // ── Paste latest stored result ──────────────────────────────
                    let latest_arc = std::sync::Arc::clone(
                        &app.state::<LatestResult>().inner().0
                    );
                    let paste_hotkey = paste_latest_hotkey_label();
                    hotkey::register_paste_callback(Arc::new(move || {
                        let text = {
                            let Ok(g) = latest_arc.lock() else { return };
                            g.clone()
                        };
                        if let Some(t) = text {
                            tracing::info!(
                                "[paste_hotkey] {} → pasting {} chars",
                                paste_hotkey,
                                t.len()
                            );
                            std::thread::spawn(move || {
                                if let Err(e) = paster::paste(&t) {
                                    tracing::warn!("[paste_hotkey] paste failed: {e}");
                                }
                            });
                        } else {
                            tracing::info!(
                                "[paste_hotkey] {} pressed but nothing stored yet",
                                paste_hotkey
                            );
                        }
                    }));
                }

                Ok(())
            }
        })
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(SharedApp(shared_app))
        .manage(BackendState(backend_arc))
        .manage(BackendHandleState(Mutex::new(None)))
        .manage(EditWatcherState(Mutex::new(None)))
        .manage(EditTargetState(Mutex::new(None)))
        .manage(ScreenContextState(Mutex::new(None)))
        .manage(RecordingRouteState(Mutex::new(None)))
        .manage(PendingProblemState(Mutex::new(None)))
        .manage(divo::DivoState::new())
        .manage(PerformanceState(Mutex::new(sysinfo::System::new_all())))
        .manage(TrayCache(Mutex::new(TrayCacheInner::default())))
        .manage(LatestResult(std::sync::Arc::new(Mutex::new(None))))
        .manage(LastActionState(Mutex::new(None)))
        .manage(LastFailedVoiceActionState(Mutex::new(None)))
        .manage(HotPathCache(Arc::new(tokio::sync::RwLock::new(HotPathCacheInner::default()))))
        .manage(StatusBarHideGen(Arc::new(AtomicU64::new(0))))
        .manage(RecordingSessionState::default())
        .manage(LiveNemotronState(Mutex::new(None)))
        .manage(StatusBarPersistentHold(AtomicBool::new(false)))
        .manage(StatusBarPlacementActive(AtomicBool::new(false)))
        .manage(StatusBarInteractive(AtomicBool::new(false)))
        .manage(MeetingModeState::new())
        .manage(meeting_engine::MeetingEngineState::new())
        .manage(speaker_suppression::SpeakerSuppressionGuard::new())
        .manage(LongDictationState::new());
    let app_result = builder
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            get_snapshot,
            chaos_inject,
            divo::divo_set_credentials,
            divo::divo_fetch_thread,
            divo::divo_send,
            divo::divo_list_threads,
            divo::divo_thread_messages,
            divo::divo_set_active_thread,
            divo_followup_begin,
            divo_followup_end,
            developer_context::developer_get_settings,
            developer_context::developer_save_settings,
            developer_context::developer_match_context,
            developer_problem_begin,
            developer_problem_end,
            developer_problem_choose_project,
            developer_problem_dismiss,
            present_status_bar,
            set_status_bar_persistent,
            dismiss_status_bar,
            resize_status_bar,
            get_status_bar_position,
            set_status_bar_position,
            reset_status_bar_position,
            set_status_bar_interactive,
            get_backend_endpoint,
            get_preferences,
            get_stt_runtime,
            get_stt_setup_policy,
            patch_preferences,
            get_history,
            get_app_icon,
            get_app_identity,
            get_app_usage,
            get_site_usage,
            get_favicon,
            trigger_browser_automation,
            request_browser_automation,
            browser_automation_status,
            submit_edit_feedback,
            toggle_recording,
            set_mode,
            request_accessibility,
            request_input_monitoring,
            request_microphone,
            open_microphone_settings,
            open_accessibility_settings,
            open_input_monitoring_settings,
            screen_recording_granted,
            request_screen_recording,
            diagnose_ax,
            // Cloud auth
            cloud_signup,
            store_enterprise_auth,
            start_enterprise_oauth_listener,
            stop_enterprise_oauth_listener,
            clear_enterprise_auth,
            get_enterprise_status,
            list_workspaces,
            get_profile_insights,
            get_app_buckets,
            set_app_bucket,
            activate_workspace,
            deactivate_workspace,
            get_device_id,
            get_hostname,
            cloud_login,
            cloud_logout,
            get_cloud_status,
            refresh_license,
            get_debug_logs,
            get_performance_snapshot,
            // Paste latest
            paste_latest,
            // Retry
            retry_recording,
            // Recording management
            delete_recording,
            get_recording_audio_url,
            get_recording_audio_bytes,
            download_recording_audio,
            export_history,
            reveal_downloaded_file,
            reveal_saved_audio,
            download_meeting_audio,
            // Pending-edit review
            get_pending_edits,
            resolve_pending_edit,
            dismiss_pending_edit,
            // Vocabulary management
            list_vocabulary,
            list_vocabulary_aliases,
            add_vocabulary_term,
            delete_vocabulary_term,
            confirm_term,
            confirm_batch,
            get_next_edit_review_session,
            skip_edit_review_session,
            block_correction,
            reset_all_vocabulary,
            star_vocabulary_term,
            patch_vocabulary_term,
            // Invite a friend
            send_invite_email,
            // OpenAI / ChatGPT OAuth
            openai_connect,
            openai_status,
            openai_disconnect,
            // External URL opener (mailto:, https://) — Tauri webview blocks window.open
            open_external,
            // Desktop-only prefs read at process startup (Sentry on/off + update channel).
            // Backed by `<data_dir>/desktop_prefs.json`, not the SQLite preferences DB.
            get_desktop_prefs,
            set_desktop_prefs,
            nemotron::nemotron_model_status,
            nemotron::download_nemotron_model,
            nemotron::delete_nemotron_model,
            // Developer log viewer
            read_backend_log,
            backend_log_location,
            open_log_folder,
            // Meeting audio pipeline
            meeting_engine_start_session,
            meeting_engine_stop_session,
            meeting_engine_toggle_mute,
            meeting_engine_get_live_transcript,
            meeting_engine_get_status,
            meeting_engine_get_processing_status,
            show_meeting_pill,
            hide_meeting_pill,
            focus_main_from_pill,
            meeting_engine::meeting_settings_get,
            meeting_engine::meeting_settings_set,
            meeting_engine::meeting_list_whisper_models,
            meeting_engine::meeting_cleanup_storage,
            meeting_engine::meeting_whisper_model_catalog,
            meeting_engine::meeting_cancel_model_download,
            meeting_engine::meeting_download_whisper_model,
            meeting_engine::download_dictation_model,
            meeting_engine::dictation_model_status,
            meeting_engine::delete_dictation_model,
            meeting_engine::reclaim_old_models,
            meeting_engine::meeting_download_silero_vad_model,
            meeting_engine::meeting_delete_silero_vad_model,
            meeting_engine::silero_vad_model_status,
            meeting_engine::meeting_delete_whisper_model,
            meeting_engine::meeting_cleanup_legacy_whisper_models,
            meeting_engine::meeting_ensure_active_model,
            meeting_engine_get_cached_artifacts,
            meeting_engine_get_cached_intelligence,
            meeting_engine_generate_intelligence,
            meeting_engine_chat,
            meeting_engine_generate_digest,
            meeting_engine_digest_chat,
            meeting_engine_list_digests,
            meeting_engine_get_digest,
            meeting_engine_delete_digest,
            meeting_engine_get_user_tags,
            meeting_engine_add_user_tag,
            meeting_engine_remove_user_tag,
            meeting_engine_get_meeting_overviews,
            meeting_engine_list_meetings,
            meeting_engine_new_local_meeting,
            meeting_engine_clear_empty_meetings,
            meeting_engine_set_meeting_title,
            meeting_engine_set_meeting_favorite,
            meeting_engine_set_meeting_hidden,
            meeting_engine_set_meeting_lark_doc,
            meeting_engine_dismiss_ai_tag,
            meeting_engine::meeting_engine_cancel_processing,
            meeting_engine_delete_meeting_files,
            meeting_engine_take_discarded_meeting_ids,
            meeting_engine_get_notes,
            meeting_engine_set_notes,
            meeting_engine_get_manual_actions,
            meeting_engine_set_manual_actions,
            meeting_engine_search_meetings,
            meeting_engine_retranscribe,
            start_meeting_stt,
            stop_meeting_stt,
            toggle_meeting_mute,
            get_meeting_stt_status,
        ])
        .build(tauri::generate_context!());

    let app = match app_result {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("[main] failed to build AirNote desktop: {e}");
            said_core::reporter::report_event(
                "desktop.build_failed",
                said_core::reporter::Severity::Fatal,
                serde_json::json!({ "error": e.to_string() }),
            );
            eprintln!("[main] failed to build AirNote desktop: {e}");
            return;
        }
    };

    app.run(|app, event| {
        // Seatbelt: the RunEvent loop runs on the AppKit main thread, so a panic
        // here would SIGABRT the app — catch + recover.
        guard_panics("run_event", || match event {
            tauri::RunEvent::Ready => {
                if launched_from_autostart() {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                    tracing::info!("[main] launch-at-login startup — main window left hidden");
                } else {
                    show_main_window(app);
                }
                #[cfg(target_os = "macos")]
                schedule_status_bar_startup_preload(app);
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } if !has_visible_windows => {
                show_main_window(app);
            }
            tauri::RunEvent::ExitRequested { code, api, .. } if code.is_none() => {
                // Window closed / Cmd+Q — hide instead of quit for accessory-app UX.
                api.prevent_exit();
            }
            tauri::RunEvent::Exit => {
                // Finalize any in-progress recording (valid WAV headers +
                // fsync + recovery breadcrumb) and stop the job worker before
                // tearing down the backend — otherwise an active recording is
                // abandoned with a stale header and the whisper child orphans.
                app.state::<meeting_engine::MeetingEngineState>().shutdown();
                if let Ok(mut guard) = app.state::<BackendHandleState>().0.lock() {
                    drop(guard.take());
                }
                backend_guard::clear_pid_file();
                tracing::info!("[main] shutdown cleanup complete — exiting process");
                exit_after_shutdown_cleanup(0);
            }
            _ => {}
        });
    });
}

// ── Tests for the meaningful-edit gate ────────────────────────────────────────

#[cfg(test)]
mod meaningful_edit_tests {
    use super::is_meaningful_edit;

    #[test]
    fn rejects_identical_after_normalize() {
        assert!(!is_meaningful_edit("Hello", "  hello  "));
    }

    #[test]
    fn rejects_punctuation_only_change() {
        assert!(!is_meaningful_edit("Hello world.", "Hello world!"));
    }

    #[test]
    fn rejects_short_non_jargon_typo() {
        // Plain prose typo within 2 chars — likely AX jitter.
        assert!(!is_meaningful_edit(
            "the meeting was good",
            "the meeting was god",
        ));
    }

    #[test]
    fn accepts_real_word_swap_at_threshold() {
        assert!(is_meaningful_edit(
            "the meeting was good",
            "the meeting was great",
        ));
    }

    #[test]
    fn accepts_short_jargon_edit_with_digits() {
        // The exact production case from logs:
        //   "Kal N10 ka IB0 nikalne wala hai." → "Kal n8n ka IB0 nikalne wala hai."
        // char_diff after normalize = 2.  Old gate rejected as "AX jitter".
        // New jargon-aware gate accepts because the changed token has digits.
        assert!(is_meaningful_edit(
            "Kal N10 ka IB0 nikalne wala hai.",
            "Kal n8n ka IB0 nikalne wala hai.",
        ));
    }

    #[test]
    fn accepts_n8n_corrections_universally() {
        assert!(is_meaningful_edit("I use 10 daily", "I use n8n daily"));
        assert!(is_meaningful_edit("I use written daily", "I use n8n daily"));
        assert!(is_meaningful_edit("I use k9s", "I use k8s")); // 1-char digit fix
        assert!(is_meaningful_edit("v2.1 release", "v2.0 release"));
    }

    #[test]
    fn accepts_brand_or_acronym_corrections_even_at_one_char() {
        assert!(is_meaningful_edit(
            "MacOps ka kitna profit hai is saal",
            "MACOBS ka kitna profit hai is saal",
        ));
    }

    #[test]
    fn rejects_zero_alphanumeric_word_changes() {
        assert!(!is_meaningful_edit("hello world", "hello   world"));
    }
}

#[cfg(test)]
mod dictation_audio_level_tests {
    use super::{dictation_pcm16_levels, dictation_wav_is_no_speech};

    fn wav_from_samples(samples: &[i16]) -> Vec<u8> {
        let data_len = samples.len() * 2;
        let mut wav = Vec::with_capacity(44 + data_len);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&16_000u32.to_le_bytes());
        wav.extend_from_slice(&32_000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data_len as u32).to_le_bytes());
        for sample in samples {
            wav.extend_from_slice(&sample.to_le_bytes());
        }
        wav
    }

    #[test]
    fn detects_no_speech_energy_in_quiet_wav() {
        let wav = wav_from_samples(&[0, 1, -1, 2, -2, 0, 1, -1]);
        let levels = dictation_wav_is_no_speech(&wav).expect("quiet wav should be no speech");
        assert!(levels.peak < 0.0035);
        assert!(levels.rms < 0.0008);
    }

    #[test]
    fn keeps_speech_like_wav() {
        let wav = wav_from_samples(&[0, 900, -900, 1200, -1200, 600, -600, 0]);
        let levels = dictation_pcm16_levels(&wav).expect("valid wav levels");
        assert!(levels.peak > 0.0035);
        assert!(dictation_wav_is_no_speech(&wav).is_none());
    }
}

#[cfg(test)]
mod edit_watch_timeout_tests {
    use super::{
        EDIT_WATCH_MAX_OBSERVATIONS, EditCaptureAnchor, EditObservationTimeline,
        clipboard_content_was_added, derive_owned_text_span, edit_watch_crossed_app_boundary,
        edit_watch_timeouts, edit_watcher_generation_is_current, effective_owned_field_value,
        extract_owned_text, new_edit_watcher_control,
    };

    #[test]
    fn gives_user_time_to_read_before_first_edit() {
        let (_max, no_edit_idle, post_edit_idle) = edit_watch_timeouts(12);
        assert!(
            no_edit_idle.as_secs() >= 20,
            "user needs time to read the pasted sentence before the first correction"
        );
        assert!(
            post_edit_idle.as_secs() >= 18,
            "after a user edit, wait long enough for reading and more corrections"
        );
    }

    #[test]
    fn long_text_gets_more_time_to_collect_multiple_edits() {
        let (max, no_edit_idle, post_edit_idle) = edit_watch_timeouts(70);
        assert!(max.as_secs() >= 60);
        assert!(no_edit_idle.as_secs() >= 25);
        assert!(post_edit_idle.as_secs() >= 25);
    }

    #[test]
    fn app_switch_closes_the_owned_edit_session() {
        assert!(!edit_watch_crossed_app_boundary(Some(101), Some(101)));
        assert!(edit_watch_crossed_app_boundary(Some(101), Some(202)));
        assert!(!edit_watch_crossed_app_boundary(Some(101), None));
        assert!(!edit_watch_crossed_app_boundary(None, Some(202)));
    }

    #[test]
    fn rapid_dictation_finalizes_old_watch_without_hard_cancel() {
        let previous = new_edit_watcher_control(7);
        previous.finalize.cancel();
        assert!(previous.finalize.is_cancelled());
        assert!(!previous.cancel.is_cancelled());

        let current = new_edit_watcher_control(8);
        assert!(!edit_watcher_generation_is_current(Some(&current), 7));
        assert!(edit_watcher_generation_is_current(Some(&current), 8));
        assert!(!edit_watcher_generation_is_current(None, 8));
    }

    #[test]
    fn detects_clipboard_text_appended_after_dictation() {
        assert!(clipboard_content_was_added(
            "Please review the proposal.",
            "Please review the proposal. https://example.com/spec",
            "https://example.com/spec",
        ));
    }

    #[test]
    fn detects_clipboard_text_prepended_before_dictation() {
        assert!(clipboard_content_was_added(
            "Please review the proposal.",
            "Context from the client: Please review the proposal.",
            "Context from the client:",
        ));
    }

    #[test]
    fn does_not_treat_word_correction_as_clipboard_addition() {
        assert!(!clipboard_content_was_added(
            "Please check the CQLite migration.",
            "Please check the SQLite migration.",
            "SQLite",
        ));
    }

    #[test]
    fn ignores_short_or_unrelated_clipboard_content() {
        assert!(!clipboard_content_was_added(
            "Please review the proposal.",
            "Please review the proposal. yes",
            "yes",
        ));
        assert!(!clipboard_content_was_added(
            "Please review the proposal.",
            "Please review the proposal carefully.",
            "unrelated clipboard text",
        ));
    }

    #[test]
    fn owned_span_comes_from_immediate_pre_and_post_snapshots() {
        let span = derive_owned_text_span(
            Some("Hello  Thanks"),
            "Hello AirNote output Thanks",
            "AirNote output",
        )
        .expect("inserted span");
        assert_eq!(span.start_chars, 6);
        assert_eq!(span.len_chars, 14);
        assert_eq!(span.text, "AirNote output");
    }

    #[test]
    fn owned_span_handles_replaced_selection_and_unicode() {
        let span =
            derive_owned_text_span(Some("Start पुराना end"), "Start नया text end", "नया text")
                .expect("replacement span");
        assert_eq!(span.start_chars, 6);
        assert_eq!(span.len_chars, 8);
        assert_eq!(span.text, "नया text");
    }

    #[test]
    fn owned_span_without_baseline_requires_unique_output() {
        assert!(
            derive_owned_text_span(None, "prefix unique output suffix", "unique output").is_some()
        );
        assert!(derive_owned_text_span(None, "same then same", "same").is_none());
    }

    #[test]
    fn owned_text_extraction_requires_unchanged_surrounding_baseline() {
        let span = derive_owned_text_span(
            Some("Before  After"),
            "Before AirNote output After",
            "AirNote output",
        );
        let anchor = EditCaptureAnchor {
            target_pid: Some(42),
            field_fingerprint: Some(7),
            pre_paste_text: Some("Before  After".to_string()),
            post_paste_text: "Before AirNote output After".to_string(),
            owned_span: span,
        };
        assert_eq!(
            extract_owned_text(&anchor, "Before corrected output After"),
            Ok("corrected output".to_string()),
        );
        assert_eq!(
            extract_owned_text(&anchor, "Changed corrected output After"),
            Err("prefix_mismatch"),
        );
        assert_eq!(
            extract_owned_text(&anchor, "Before corrected output Changed"),
            Err("suffix_mismatch"),
        );
    }

    #[test]
    fn observation_timeline_preserves_multiple_owned_edits() {
        let anchor = EditCaptureAnchor {
            target_pid: Some(42),
            field_fingerprint: Some(7),
            pre_paste_text: Some("Before  After".to_string()),
            post_paste_text: "Before CQLite and MacOps After".to_string(),
            owned_span: derive_owned_text_span(
                Some("Before  After"),
                "Before CQLite and MacOps After",
                "CQLite and MacOps",
            ),
        };
        let mut timeline = EditObservationTimeline::default();
        assert_eq!(
            timeline.record(
                &anchor,
                &anchor.post_paste_text,
                "Before SQLite and MacOps After",
                500,
            ),
            Ok(true),
        );
        assert_eq!(
            timeline.record(
                &anchor,
                &anchor.post_paste_text,
                "Before SQLite and MACOBS After",
                900,
            ),
            Ok(true),
        );
        assert_eq!(timeline.total_observations, 2);
        assert_eq!(timeline.retained_count(), 2);
        assert_eq!(timeline.elapsed_span_ms(), Some(400));
        assert_eq!(
            timeline.latest_field_value(),
            Some("Before SQLite and MACOBS After"),
        );
        assert_eq!(
            effective_owned_field_value(&anchor, "Before SQLite and MACOBS After", &timeline,),
            "Before SQLite and MACOBS After",
        );
    }

    #[test]
    fn field_change_finalizes_prior_owned_edit_without_reading_new_field() {
        let anchor = EditCaptureAnchor {
            target_pid: Some(42),
            field_fingerprint: Some(7),
            pre_paste_text: None,
            post_paste_text: "Original output".to_string(),
            owned_span: derive_owned_text_span(None, "Original output", "Original output"),
        };
        let mut timeline = EditObservationTimeline::default();
        timeline
            .record(&anchor, &anchor.post_paste_text, "Corrected output", 500)
            .unwrap();

        assert_eq!(
            effective_owned_field_value(&anchor, "Corrected output", &timeline),
            "Corrected output",
        );
    }

    #[test]
    fn observation_timeline_is_bounded_and_rejects_unowned_changes() {
        let anchor = EditCaptureAnchor {
            target_pid: Some(42),
            field_fingerprint: Some(7),
            pre_paste_text: Some("Before  After".to_string()),
            post_paste_text: "Before original After".to_string(),
            owned_span: derive_owned_text_span(
                Some("Before  After"),
                "Before original After",
                "original",
            ),
        };
        let mut timeline = EditObservationTimeline::default();
        for index in 0..EDIT_WATCH_MAX_OBSERVATIONS + 5 {
            let value = format!("Before edit-{index} After");
            assert_eq!(
                timeline.record(&anchor, &anchor.post_paste_text, &value, index as u64,),
                Ok(true),
            );
        }
        assert_eq!(timeline.retained_count(), EDIT_WATCH_MAX_OBSERVATIONS);
        assert_eq!(timeline.dropped_count(), 5);
        assert_eq!(
            timeline.record(
                &anchor,
                &anchor.post_paste_text,
                "Changed edit outside the anchor After",
                100,
            ),
            Err("prefix_mismatch"),
        );
    }
}
