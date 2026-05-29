#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod backend;
mod backend_guard;
mod desktop;
mod dg_stream; // P5: Deepgram WebSocket live streaming
mod enterprise_oauth;
// mod meeting_audio; // Removed: meeting mode reuses the main pipeline
mod permissions;

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{
    Emitter, Manager, State,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
};
use tokio_util::sync::CancellationToken;

use backend::BackendEndpoint;
use desktop::DesktopApp;
use said_core::{AppSnapshot, ProcessSummary};
use said_paster as paster;

const DEBUG_LOG_MAX_BYTES: u64 = 240_000;
const STREAM_RESET_SENTINEL: &str = "\u{1F}__RESET__\u{1F}";
const MEETING_SPEECH_LEVEL: f32 = 0.025;
const MEETING_SILENCE_LEVEL: f32 = 0.012;
const MEETING_PAUSE_MS: u64 = 900;
const MEETING_MIN_CHUNK_MS: u64 = 700;
const MEETING_MAX_CHUNK_MS: u64 = 30_000;

fn is_short_recording_cancel(err: &str) -> bool {
    err == desktop::RECORDING_TOO_SHORT_ERROR
}

fn record_hotkey_label(raw: &str) -> &'static str {
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
}

#[cfg(target_os = "macos")]
fn status_bar_collection_behavior() -> CollectionBehavior {
    CollectionBehavior::new()
        .can_join_all_spaces()
        .stationary()
        .ignores_cycle()
        .full_screen_auxiliary()
}

#[cfg(target_os = "macos")]
fn tune_status_bar_panel(app: &tauri::AppHandle) {
    let Ok(panel) = app.get_webview_panel("status-bar") else {
        return;
    };
    panel.set_level(PanelLevel::Custom(28).value());
    panel.set_floating_panel(true);
    panel.set_hides_on_deactivate(false);
    panel.set_works_when_modal(true);
    panel.set_ignores_mouse_events(true);
    panel.set_collection_behavior(status_bar_collection_behavior().into());
    panel.set_style_mask(StyleMask::empty().borderless().nonactivating_panel().into());
    panel.set_transparent(true);
    panel.set_has_shadow(false);
}

#[cfg(target_os = "macos")]
fn show_status_bar_panel(app: &tauri::AppHandle) -> bool {
    match app.get_webview_panel("status-bar") {
        Ok(panel) => {
            tune_status_bar_panel(app);
            panel.show();
            panel.order_front_regardless();
            true
        }
        Err(_) => {
            tracing::warn!("[status-bar] panel handle missing; falling back to webview window");
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn configure_status_bar_macos(win: &tauri::WebviewWindow) {
    use objc::Message;
    use objc::runtime::{Object, Sel};

    let Ok(ns_window) = win.ns_window() else {
        tracing::warn!("[status-bar] macOS tune failed: ns_window unavailable");
        return;
    };
    if ns_window.is_null() {
        tracing::warn!("[status-bar] macOS tune failed: ns_window was null");
        return;
    }

    // Match VoiceInk's recorder HUD window behavior as closely as Tauri's
    // NSWindow allows: a non-activating floating panel, available on every
    // Space, allowed over fullscreen apps, stationary during Space transitions,
    // and kept out of Cmd-` window cycling.
    const CAN_JOIN_ALL_SPACES: usize = 1 << 0;
    const STATIONARY: usize = 1 << 4;
    const IGNORES_CYCLE: usize = 1 << 6;
    const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
    const NONACTIVATING_PANEL_STYLE: usize = 1 << 7;
    const FULL_SIZE_CONTENT_VIEW_STYLE: usize = 1 << 15;
    const NS_STATUS_WINDOW_LEVEL_PLUS_THREE: isize = 28;

    unsafe {
        let ns_window = &*(ns_window as *mut Object);
        let style_mask: usize = ns_window
            .send_message(Sel::register("styleMask"), ())
            .unwrap_or(0);
        let panel_style = style_mask | NONACTIVATING_PANEL_STYLE | FULL_SIZE_CONTENT_VIEW_STYLE;
        let _: Result<(), _> =
            ns_window.send_message(Sel::register("setStyleMask:"), (panel_style,));

        let behavior = CAN_JOIN_ALL_SPACES | STATIONARY | IGNORES_CYCLE | FULL_SCREEN_AUXILIARY;
        let _: Result<(), _> =
            ns_window.send_message(Sel::register("setCollectionBehavior:"), (behavior,));
        let _: Result<(), _> = ns_window.send_message(
            Sel::register("setLevel:"),
            (NS_STATUS_WINDOW_LEVEL_PLUS_THREE,),
        );
        let _: Result<(), _> = ns_window.send_message(Sel::register("setCanHide:"), (false,));
        let _: Result<(), _> =
            ns_window.send_message(Sel::register("setIgnoresMouseEvents:"), (true,));
        for (selector_name, value) in [
            ("setHidesOnDeactivate:", false),
            ("setFloatingPanel:", true),
        ] {
            let selector = Sel::register(selector_name);
            let responds: bool = ns_window
                .send_message(Sel::register("respondsToSelector:"), (selector,))
                .unwrap_or(false);
            if responds {
                let _: Result<(), _> = ns_window.send_message(selector, (value,));
            }
        }
        let _: Result<(), _> = ns_window.send_message(Sel::register("orderFrontRegardless"), ());
        tracing::debug!(
            "[status-bar] macOS tuned style={panel_style:#x} behavior={behavior:#x} level={NS_STATUS_WINDOW_LEVEL_PLUS_THREE}"
        );
    }
}

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
fn status_bar_pinned() -> bool {
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
) {
    reposition_status_bar(app, win);
    configure_status_bar_macos(win);
    if let Err(e) = win.set_always_on_top(true) {
        tracing::warn!("[status-bar] set_always_on_top failed: {e}");
    }
    match win.show() {
        Ok(_) => tracing::debug!("[status-bar] show ok for state={state}"),
        Err(e) => tracing::warn!("[status-bar] show failed for state={state}: {e}"),
    }
    configure_status_bar_macos(win);
    let _ = show_status_bar_panel(app);
}

#[cfg(target_os = "macos")]
fn schedule_present_status_bar_macos(
    app: &tauri::AppHandle,
    win: &tauri::WebviewWindow,
    state: &str,
) {
    let app_for_main = app.clone();
    let app_in_closure = app_for_main.clone();
    let win = win.clone();
    let state = state.to_string();
    if let Err(e) = app_for_main.run_on_main_thread(move || {
        present_status_bar_macos_on_main(&app_in_closure, &win, &state);
    }) {
        tracing::warn!("[status-bar] schedule present on main thread failed: {e}");
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
/// We never want to surface diagnostic text like "Deepgram error 400:
/// invalid audio format" or "empty transcript — nothing spoken?" to the
/// user — those read like log lines.  This function maps the common cases
/// to plain English and falls back to a generic apology for everything
/// else.  Keep wording calm and blame-free; no "Error:" prefix, no codes,
/// no emoji.
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
    let lower = raw.to_lowercase();

    if lower.contains("empty transcript") || lower.contains("nothing spoken") {
        return "Couldn't hear anything".into();
    }
    if lower.contains("recording interrupted") {
        return "Recording interrupted".into();
    }
    if lower.contains("deepgram")
        && (lower.contains("401") || lower.contains("403") || lower.contains("unauthorized"))
    {
        return "Deepgram key invalid".into();
    }
    if lower.contains("deepgram") && (lower.contains("429") || lower.contains("rate")) {
        return "STT rate limited".into();
    }
    if lower.contains("deepgram") || lower.contains("stt") {
        return "STT failed — try again".into();
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LiveTypingDecision {
    ResetAndDisable,
    PreviewOnly,
    TypeToken,
}

#[derive(Debug, Default)]
struct LiveTypingGuard {
    disabled: bool,
}

impl LiveTypingGuard {
    fn on_token(&mut self, token: &str) -> LiveTypingDecision {
        if token == STREAM_RESET_SENTINEL {
            self.disabled = true;
            return LiveTypingDecision::ResetAndDisable;
        }
        if self.disabled {
            return LiveTypingDecision::PreviewOnly;
        }
        LiveTypingDecision::TypeToken
    }
}

// ── Managed state ─────────────────────────────────────────────────────────────

/// Holds the local recording state machine.
struct SharedApp(Arc<Mutex<DesktopApp>>);

/// Holds the backend endpoint (url + secret). None until daemon starts.
struct BackendState(Arc<Mutex<Option<BackendEndpoint>>>);

/// Owns the BackendHandle (and its Child) for the lifetime of the app.
/// When Tauri drops managed state on exit, Drop fires → SIGTERM → SIGKILL.
struct BackendHandleState(Mutex<Option<backend::BackendHandle>>);

/// Owns the currently running post-paste edit watcher. Starting a new watcher
/// cancels the previous one so rapid recordings cannot stack poll loops.
struct EditWatcherState(Mutex<Option<CancellationToken>>);

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

/// P5: Holds the oneshot receiver that delivers the pre-transcript from the
/// Deepgram WebSocket streaming task.  Replaced on every new recording.
struct StreamingState(
    Mutex<Option<tokio::sync::oneshot::Receiver<Option<dg_stream::StreamingTranscript>>>>,
);

struct DeepgramSessionState(dg_stream::SessionSender);

/// Stores the most-recently polished text. Populated after every voice/text polish;
/// cleared after it's pasted via Ctrl+Cmd+V or the `paste_latest` Tauri command.
struct LatestResult(std::sync::Arc<Mutex<Option<String>>>);

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

struct PerformanceState(Mutex<sysinfo::System>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordingRoute {
    Normal,
    Meeting,
}

struct RecordingRouteState(Mutex<Option<RecordingRoute>>);

struct LongDictationState {
    locked: Arc<AtomicBool>,
    pending_lock: Arc<AtomicBool>,
    stop_consumed: Arc<AtomicBool>,
}

impl LongDictationState {
    fn new() -> Self {
        Self {
            locked: Arc::new(AtomicBool::new(false)),
            pending_lock: Arc::new(AtomicBool::new(false)),
            stop_consumed: Arc::new(AtomicBool::new(false)),
        }
    }

    fn reset(&self) {
        self.locked.store(false, Ordering::SeqCst);
        self.pending_lock.store(false, Ordering::SeqCst);
        self.stop_consumed.store(false, Ordering::SeqCst);
    }
}

#[derive(Clone, Serialize)]
struct MeetingSttStatus {
    active: bool,
    muted: bool,
    capture_running: bool,
}

/// Meeting-mode flags. Active means the live meeting view owns the recorder;
/// muted prevents pause-detection restarts while the user is holding Fn.
struct MeetingModeState {
    active: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    generation: Arc<AtomicU64>,
}

impl MeetingModeState {
    fn new() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            muted: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(AtomicU64::new(0)),
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
    }

    fn status(&self) -> MeetingSttStatus {
        MeetingSttStatus {
            active: self.is_active(),
            muted: self.is_muted(),
            capture_running: self.capture_enabled(),
        }
    }
}

fn emit_meeting_stt_status(app: &tauri::AppHandle) {
    if let Some(meeting) = app.try_state::<MeetingModeState>() {
        let _ = app.emit("meeting-stt-state", meeting.status());
    }
}

/// Hot-path cache: language setting + personal vocabulary keyterms.
///
/// Populated once when the backend becomes ready; refreshed in the background
/// whenever preferences change or new vocabulary is learned.  The WS recording
/// task reads from this cache — zero HTTP calls on the recording critical path.
struct HotPathCache(Arc<tokio::sync::RwLock<HotPathCacheInner>>);

#[derive(Default, Clone)]
struct HotPathCacheInner {
    /// User's STT language setting (e.g. "hi", "multi", "auto").
    language: String,
    /// Saved Deepgram API key from preferences.
    deepgram_key: String,
    /// Resolved STT mode sent to Deepgram.
    stt_mode: String,
    /// Personal vocabulary terms sent to Deepgram as `keyterm=` biases.
    keyterms: Vec<String>,
    /// Trusted upstream replacement rules sent as `replace=`.
    replacements: Vec<said_core::deepgram::ReplacementRule>,
}

/// Generation counter for status-bar idle hide timers.
/// Each idle sync increments this; a timer whose generation no longer matches is silently dropped.
struct StatusBarHideGen(Arc<AtomicU64>);

/// True while ⇧⌘/ placement mode is active (drag to reposition HUD).
struct StatusBarPlacementActive(AtomicBool);

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
    desktop: String,
    backend: String,
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

fn parse_record_hotkey(raw: &str) -> hotkey::RecordHotkey {
    match raw {
        "right_option" => hotkey::RecordHotkey::RightOption,
        "fn" => hotkey::RecordHotkey::Function,
        _ => hotkey::RecordHotkey::CapsLock,
    }
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
    // ── 1. Toggle recording (state-aware label + enabled) ──────────────
    let toggle_label = match snap.state.as_str() {
        "recording" => "Stop recording",
        _ => "Start recording",
    };
    let toggle_enabled = snap.state.as_str() == "idle";
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

    let p_format = MenuItem::with_id(app, "tray_polish_format", h_format, true, None::<&str>)?;
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
    let win = match handle.get_webview_window("status-bar") {
        Some(win) => win,
        None if state != "idle" => {
            tracing::warn!(
                "[status-bar] sync requested for active state={state}, but window was not found — recreating"
            );
            create_status_bar(handle);
            let Some(win) = handle.get_webview_window("status-bar") else {
                tracing::warn!(
                    "[status-bar] recreate failed; still no status-bar window for state={state}"
                );
                return;
            };
            win
        }
        None => {
            tracing::warn!(
                "[status-bar] sync requested for state={state}, but window was not found"
            );
            return;
        }
    };

    tracing::debug!("[status-bar] sync state={state}");
    if state == "idle" {
        if status_bar_pinned() {
            tracing::debug!("[status-bar] idle state — pinned, keeping visible");
            #[cfg(target_os = "macos")]
            schedule_present_status_bar_macos(handle, &win, state);
            #[cfg(not(target_os = "macos"))]
            {
                let _ = win.set_always_on_top(true);
                let _ = win.set_visible_on_all_workspaces(true);
                let _ = win.show();
            }
            return;
        }
        tracing::debug!("[status-bar] idle state — scheduling native hide");
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
            if let Some(win) = app.get_webview_window("status-bar") {
                match win.hide() {
                    Ok(_) => tracing::debug!("[status-bar] hidden after idle"),
                    Err(e) => tracing::warn!("[status-bar] hide after idle failed: {e}"),
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

    #[cfg(target_os = "macos")]
    {
        schedule_present_status_bar_macos(handle, &win, state);
    }

    #[cfg(not(target_os = "macos"))]
    {
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
    }
}

/// Re-render the tray icon title + menu from the cached prefs (no async needed).
fn sync_tray(handle: &tauri::AppHandle, snap: &AppSnapshot) {
    if let Some(tray) = handle.tray_by_id("said") {
        let _ = tray.set_title(Some(tray_title(&snap.state)));

        // Read from in-process cache — never blocks on async or HTTP
        let cache = handle.state::<TrayCache>();
        let inner = cache.0.lock().unwrap_or_else(|p| p.into_inner());
        let custom = inner.custom_prompt.clone();
        let lang = inner.output_language.clone();
        drop(inner);

        if let Ok(menu) = build_tray_menu(handle, snap, custom.as_deref(), &lang) {
            let _ = tray.set_menu(Some(menu));
        }
    }

    sync_status_bar(handle, &snap.state);
}

// ── Floating status bar ───────────────────────────────────────────────────────

/// Create the always-on-top floating status pill.
///
/// The window loads the same SPA with an explicit statusbar marker so
/// `main.tsx` renders `<StatusBar />` instead of the full app. It starts hidden
/// at idle and is shown by `sync_status_bar()` when recording/processing begins.
fn create_status_bar(app: &tauri::AppHandle) {
    if app.get_webview_window("status-bar").is_some() {
        tracing::info!("[status-bar] create skipped; window already exists");
        return;
    }

    // Position: bottom-center, low above the dock. Match VoiceInk's panel model:
    // keep a max-size transparent native canvas and expand the inner HUD inside it.
    let idle_w = STATUS_BAR_WIDTH;
    let idle_h = STATUS_BAR_HEIGHT;
    let (x, y) = status_bar_target_origin(app, idle_w, idle_h);

    let url = "index.html?view=statusbar#statusbar";
    tracing::info!(
        "[status-bar] creating window url={url} x={x:.0} y={y:.0} size={idle_w:.0}x{idle_h:.0} visible=false"
    );

    #[cfg(target_os = "macos")]
    {
        match PanelBuilder::<_, StatusBarPanel>::new(app, "status-bar")
            .url(tauri::WebviewUrl::App(url.into()))
            .title("AirNote")
            .size(tauri::Size::Logical(tauri::LogicalSize::new(
                idle_w, idle_h,
            )))
            .position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
            .level(PanelLevel::Custom(28))
            .floating(true)
            .hides_on_deactivate(false)
            .works_when_modal(true)
            .ignores_mouse_events(true)
            .has_shadow(false)
            .transparent(true)
            .style_mask(StyleMask::empty().borderless().nonactivating_panel())
            .collection_behavior(status_bar_collection_behavior())
            .no_activate(true)
            .with_window(|window| {
                window
                    .decorations(false)
                    .always_on_top(true)
                    .visible_on_all_workspaces(true)
                    .skip_taskbar(true)
                    .focused(false)
                    .resizable(true)
                    .shadow(false)
                    .transparent(true)
                    .visible(false)
            })
            .build()
        {
            Ok(panel) => {
                tracing::info!("[status-bar] NSPanel created label={}", panel.label());
                tune_status_bar_panel(app);
                if status_bar_pinned() {
                    tracing::info!("[status-bar] dev pin active — showing at idle");
                } else {
                    panel.hide();
                }
                if let Some(win) = app.get_webview_window("status-bar") {
                    match win.url() {
                        Ok(url) => tracing::info!("[status-bar] resolved url={url}"),
                        Err(e) => tracing::warn!("[status-bar] could not read window url: {e}"),
                    }
                    let _ = win.set_ignore_cursor_events(true);
                    configure_status_bar_macos(&win);
                }
                sync_status_bar(app, "idle");
            }
            Err(e) => tracing::warn!("[status-bar] could not create NSPanel: {e}"),
        }
        return;
    }

    #[cfg(not(target_os = "macos"))]
    match tauri::WebviewWindowBuilder::new(app, "status-bar", tauri::WebviewUrl::App(url.into()))
        .title("AirNote")
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
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
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
        long.reset();
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

fn insert_text_prefer_direct(label: &str, text: &str) -> Result<(), String> {
    match paster::type_text(text) {
        Ok(true) => Ok(()),
        Ok(false) => {
            tracing::warn!("[{label}] direct typing unavailable — falling back to clipboard paste");
            paster::paste(text)
        }
        Err(e) => {
            tracing::warn!("[{label}] direct typing failed: {e} — falling back to clipboard paste");
            paster::paste(text)
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
                let _ = app_clone.emit("voice-done", &done);
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
    Ok(snap)
}

#[tauri::command]
fn get_snapshot(state: State<'_, SharedApp>) -> Result<AppSnapshot, String> {
    Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
}

#[tauri::command]
fn dismiss_status_bar(app: tauri::AppHandle) -> Result<(), String> {
    if status_bar_pinned() {
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

fn origin_from_bottom_anchor(center_x: f64, bottom_y: f64, width: f64, height: f64) -> (f64, f64) {
    (center_x - width / 2.0, bottom_y - height)
}

#[tauri::command]
fn resize_status_bar(app: tauri::AppHandle, width: f64, height: f64) -> Result<(), String> {
    let win = app
        .get_webview_window("status-bar")
        .ok_or_else(|| "status-bar window not found".to_string())?;
    let (center_x, bottom_y) = read_window_bottom_anchor(&win).unwrap_or_else(|| {
        let (x, y) = status_bar_target_origin(&app, width, height);
        (x + width / 2.0, y + height)
    });
    win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)))
        .map_err(|e| format!("resize status bar failed: {e}"))?;
    let (x, y) = origin_from_bottom_anchor(center_x, bottom_y, width, height);
    win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
        .map_err(|e| format!("post-resize position failed: {e}"))?;
    Ok(())
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

#[tauri::command]
fn reset_status_bar_position(app: tauri::AppHandle) -> Result<(), String> {
    clear_status_bar_position()?;
    if let Some(win) = app.get_webview_window("status-bar") {
        apply_status_bar_position(&app, &win)?;
    }
    Ok(())
}

#[tauri::command]
fn set_status_bar_interactive(app: tauri::AppHandle, interactive: bool) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("status-bar") {
        let _ = win.set_ignore_cursor_events(!interactive);
        #[cfg(target_os = "macos")]
        {
            use objc::Message;
            use objc::runtime::{Object, Sel};
            if let Ok(ns_window) = win.ns_window() {
                if !ns_window.is_null() {
                    unsafe {
                        let ns_window = &*(ns_window as *mut Object);
                        let _: Result<(), _> = ns_window
                            .send_message(Sel::register("setIgnoresMouseEvents:"), (!interactive,));
                    }
                }
            }
        }
        tracing::debug!("[status-bar] interactive={interactive}");
    }
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

#[tauri::command]
async fn get_voice_prompt(
    backend: State<'_, BackendState>,
) -> Result<api::PromptTemplateResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::get_voice_prompt(&ep).await
}

#[tauri::command]
async fn save_voice_prompt_draft(
    backend: State<'_, BackendState>,
    draft_body: String,
) -> Result<api::PromptTemplateResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::save_voice_prompt_draft(&ep, draft_body).await
}

#[tauri::command]
async fn apply_voice_prompt_draft(
    backend: State<'_, BackendState>,
) -> Result<api::PromptTemplateResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::apply_voice_prompt_draft(&ep).await
}

#[tauri::command]
async fn reset_voice_prompt(
    backend: State<'_, BackendState>,
) -> Result<api::PromptTemplateResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::reset_voice_prompt(&ep).await
}

#[tauri::command]
async fn test_voice_prompt(
    backend: State<'_, BackendState>,
    transcript: String,
    draft_body: Option<String>,
) -> Result<api::PromptTestResponse, String> {
    let ep = get_endpoint(&backend)?;
    api::test_voice_prompt(&ep, transcript, draft_body).await
}

#[tauri::command]
async fn patch_preferences(
    backend: State<'_, BackendState>,
    tray_cache: State<'_, TrayCache>,
    hot_cache: State<'_, HotPathCache>,
    dg_session: State<'_, DeepgramSessionState>,
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
            hot.deepgram_key = p.deepgram_api_key.clone().unwrap_or_default();
        }
        Err(e) => tracing::warn!("[patch_prefs] backend error: {e}"),
    }
    if result.is_ok() {
        let arc = Arc::clone(&hot_cache.0);
        let session_tx = dg_session.0.clone();
        let ep2 = ep.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(bias) = api::get_stt_bias(&ep2).await {
                let mut hot = arc.write().await;
                hot.stt_mode = bias.stt_mode;
                hot.keyterms = bias.keyterms;
                hot.replacements = bias.replacements;
                let deepgram_key = hot.deepgram_key.clone();
                let session_bias = said_core::deepgram::BiasPackage {
                    stt_mode: hot.stt_mode.clone(),
                    keyterms: hot.keyterms.clone(),
                    replacements: hot.replacements.clone(),
                };
                drop(hot);
                let _ = session_tx
                    .send(dg_stream::SessionCommand::Reconfigure {
                        deepgram_key,
                        bias: session_bias,
                    })
                    .await;
            }
        });
    }
    result
}

#[tauri::command]
async fn get_history(
    backend: State<'_, BackendState>,
    limit: Option<i64>,
) -> Result<Vec<api::Recording>, String> {
    let ep = get_endpoint(&backend)?;
    api::get_history(&ep, limit.unwrap_or(50)).await
}

#[tauri::command]
async fn submit_edit_feedback(
    backend: State<'_, BackendState>,
    hot_cache: State<'_, HotPathCache>,
    dg_session: State<'_, DeepgramSessionState>,
    recording_id: String,
    user_kept: String,
    target_app: Option<String>,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let result = api::submit_feedback(&ep, &recording_id, &user_kept, target_app.as_deref()).await;
    // After feedback the backend may have learned new vocabulary — refresh
    // keyterms in the background so the next recording already has them.
    if result.is_ok() {
        let arc = Arc::clone(&hot_cache.0);
        let session_tx = dg_session.0.clone();
        let ep2 = ep.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(bias) = api::get_stt_bias(&ep2).await {
                tracing::debug!(
                    "[hot_cache] refreshed STT bias after feedback (mode={} keyterms={} replacements={})",
                    bias.stt_mode,
                    bias.keyterms.len(),
                    bias.replacements.len()
                );
                let mut hot = arc.write().await;
                hot.stt_mode = bias.stt_mode;
                hot.keyterms = bias.keyterms;
                hot.replacements = bias.replacements;
                let deepgram_key = hot.deepgram_key.clone();
                let session_bias = said_core::deepgram::BiasPackage {
                    stt_mode: hot.stt_mode.clone(),
                    keyterms: hot.keyterms.clone(),
                    replacements: hot.replacements.clone(),
                };
                drop(hot);
                let _ = session_tx
                    .send(dg_stream::SessionCommand::Reconfigure {
                        deepgram_key,
                        bias: session_bias,
                    })
                    .await;
            }
        });
    }
    result
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
    Ok(snap)
}

#[tauri::command]
fn request_accessibility(state: State<'_, SharedApp>) -> Result<AppSnapshot, String> {
    paster::request_permission();
    Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
}

#[tauri::command]
fn request_input_monitoring(state: State<'_, SharedApp>) -> Result<AppSnapshot, String> {
    paster::request_input_monitoring();
    Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
}

#[tauri::command]
fn request_microphone(state: State<'_, SharedApp>) -> Result<AppSnapshot, String> {
    permissions::request_microphone();
    Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
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
            Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
        }
        desktop::AppState::Recording => {
            do_finish_recording(Arc::clone(&state.0), app.clone(), Arc::clone(&backend.0));
            Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
        }
        desktop::AppState::Processing => {
            // Already in flight — return current snapshot, don't do anything
            Ok(state.0.lock().map_err(|_| "lock failed")?.snapshot())
        }
    }
}

// ── Recording flow ────────────────────────────────────────────────────────────

/// Guards against overlapping start-start or start-cancel-start races.
/// The start→finish pair is allowed through (finish clears the flag),
/// but a second START while the first hasn't finished is rejected.
static RECORDING_STARTING: AtomicBool = AtomicBool::new(false);
static HOTKEY_START_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static FINISH_AFTER_START: AtomicBool = AtomicBool::new(false);

/// Minimum time between consecutive finish→start cycles (ms).
/// Prevents rapid Caps Lock taps from flooding the recording pipeline.
static LAST_FINISH_MS: AtomicU64 = AtomicU64::new(0);
const MIN_CYCLE_GAP_MS: u64 = 300;

fn now_ms_desktop() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn hotkey_current_state(shared: &Arc<Mutex<DesktopApp>>, label: &str) -> Option<desktop::AppState> {
    for attempt in 0..10 {
        if let Ok(d) = shared.try_lock() {
            return Some(d.state);
        }
        if attempt == 0 {
            tracing::debug!("[hotkey] {label} waiting for shared app lock");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    tracing::warn!("[hotkey] {label} skipped — shared app lock busy for 200ms");
    None
}

/// Start recording. Called when user presses Caps Lock (or taps the button).
fn do_start_recording(shared: &Arc<Mutex<DesktopApp>>, app: &tauri::AppHandle) {
    // Reject if another start is already in progress
    if RECORDING_STARTING.swap(true, Ordering::SeqCst) {
        tracing::info!("[record] start skipped — another start already in progress");
        return;
    }

    // Reject rapid re-entry after a recent finish
    let now = now_ms_desktop();
    let last_finish = LAST_FINISH_MS.load(Ordering::SeqCst);
    if last_finish > 0 && now.saturating_sub(last_finish) < MIN_CYCLE_GAP_MS {
        tracing::info!(
            "[record] start skipped — too soon after last finish ({}ms < {}ms)",
            now.saturating_sub(last_finish),
            MIN_CYCLE_GAP_MS,
        );
        FINISH_AFTER_START.store(false, Ordering::SeqCst);
        RECORDING_STARTING.store(false, Ordering::SeqCst);
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

    cancel_edit_watcher(app, "new recording");

    // Lock and pre-unlock the frontmost app's AX tree BEFORE recording begins.
    // Chrome / Electron need ~150-200 ms to build their accessibility cache after
    // AXEnhancedUserInterface / AXManualAccessibility is set.  By unlocking here
    // we give the browser the full dictation window (typically 2-10 s) to get
    // ready, so that post-paste edit detection can read AXValue reliably.
    #[cfg(target_os = "macos")]
    {
        let meeting_capture = app
            .try_state::<MeetingModeState>()
            .map(|s| s.capture_enabled())
            .unwrap_or(false);
        if !meeting_capture {
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
                if ctx.is_some() {
                    tracing::info!(
                        "[record] screen context: {} chars",
                        ctx.as_ref().unwrap().len()
                    );
                }
            }
        }
    }

    let (started, level_recv) = match shared.lock() {
        Ok(mut d) => {
            let result = d.start_recording();
            let lr = if result.is_ok() {
                d.take_level_receiver()
            } else {
                None
            };
            (result, lr)
        }
        Err(_) => return,
    };
    match started {
        Ok(snap) => {
            let route = app
                .try_state::<MeetingModeState>()
                .map(|s| {
                    if s.capture_enabled() {
                        RecordingRoute::Meeting
                    } else {
                        RecordingRoute::Normal
                    }
                })
                .unwrap_or(RecordingRoute::Normal);
            if let Ok(mut route_state) = app.state::<RecordingRouteState>().0.lock() {
                *route_state = Some(route);
            }
            tracing::info!("[record] started — state={}", snap.state);
            sync_tray(app, &snap);
            let _ = app.emit("app-state", &snap);
            if app
                .try_state::<LongDictationState>()
                .map(|s| s.pending_lock.swap(false, Ordering::SeqCst))
                .unwrap_or(false)
            {
                activate_long_dictation_lock(app);
            }
            emit_meeting_stt_status(app);
        }
        Err(e) => {
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
    }

    // Drive the floating HUD visualizer from the same microphone samples used
    // by recording. This stays independent from Deepgram so the UI remains
    // responsive even if streaming is disabled or falls back to HTTP STT.
    let level_recv = level_recv;
    if let Some(level_recv) = level_recv {
        let app_levels = app.clone();
        let meeting_pause = app.try_state::<MeetingModeState>().and_then(|meeting| {
            if !meeting.capture_enabled() {
                return None;
            }
            Some((
                Arc::clone(&meeting.active),
                Arc::clone(&meeting.muted),
                Arc::clone(&meeting.generation),
                meeting.generation.load(Ordering::SeqCst),
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
            while let Ok(level) = level_recv.rx.recv() {
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

                if let Some((active, muted, generation, expected_generation, shared, _, _)) =
                    &meeting_pause
                {
                    if !active.load(Ordering::SeqCst)
                        || muted.load(Ordering::SeqCst)
                        || generation.load(Ordering::SeqCst) != *expected_generation
                    {
                        break;
                    }

                    let now = std::time::Instant::now();
                    if level >= MEETING_SPEECH_LEVEL {
                        heard_speech = true;
                        last_voice_at = now;
                    } else if heard_speech
                        && level <= MEETING_SILENCE_LEVEL
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
                if let Some((_, _, _, _, shared, app, backend)) = meeting_pause {
                    do_finish_recording(shared, app, backend);
                }
            }
        });
    }

    // ── Meeting time ceiling: force-finalize after MEETING_MAX_CHUNK_MS ────────
    // Prevents unbounded RAM accumulation when someone talks non-stop.
    // Must fire before Deepgram's 45s MAX_STREAMING_DURATION to avoid data loss.
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

    // ── P5: Start Deepgram WS streaming immediately ────────────────────────────
    let chunk_recv = shared.lock().ok().and_then(|mut d| d.take_chunk_receiver());
    if let Some(chunk_recv) = chunk_recv {
        let recording_id = uuid::Uuid::new_v4().to_string();
        let streaming_state = app.state::<StreamingState>();
        let (transcript_tx, transcript_rx) =
            tokio::sync::oneshot::channel::<Option<dg_stream::StreamingTranscript>>();
        if let Some(mut g) = streaming_state.0.lock().ok() {
            *g = Some(transcript_rx);
        }
        let utterance_end_tx = app.try_state::<MeetingModeState>().and_then(|meeting| {
            if !meeting.capture_enabled() {
                return None;
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let active = Arc::clone(&meeting.active);
            let muted = Arc::clone(&meeting.muted);
            let expected_generation = meeting.generation.load(Ordering::SeqCst);
            let generation = Arc::clone(&meeting.generation);
            let shared = Arc::clone(shared);
            let app = app.clone();
            let backend = Arc::clone(&app.state::<BackendState>().0);
            tauri::async_runtime::spawn(async move {
                if rx.recv().await.is_some()
                    && active.load(Ordering::SeqCst)
                    && !muted.load(Ordering::SeqCst)
                    && generation.load(Ordering::SeqCst) == expected_generation
                {
                    let still_recording = hotkey_current_state(&shared, "meeting utterance end")
                        == Some(desktop::AppState::Recording);
                    if still_recording {
                        tracing::info!("[meeting_mode] Deepgram utterance end — finishing chunk");
                        do_finish_recording(shared, app, backend);
                    }
                }
            });
            Some(tx)
        });

        let backend_for_pe = Arc::clone(&app.state::<BackendState>().0);
        let session_tx = app.state::<DeepgramSessionState>().0.clone();

        tauri::async_runtime::spawn(async move {
            let pre_embed_info: Option<(String, String)> =
                backend_for_pe.lock().ok().and_then(|g| {
                    g.as_ref()
                        .map(|ep| (format!("{}/v1/pre-embed", ep.url), ep.secret.clone()))
                });
            let start_cmd = dg_stream::SessionCommand::StartRecording {
                id: recording_id.clone(),
                result_tx: transcript_tx,
                pre_embed: pre_embed_info,
                utterance_end_tx,
            };
            if let Err(err) = session_tx.send(start_cmd).await {
                if let dg_stream::SessionCommand::StartRecording { result_tx, .. } = err.0 {
                    let _ = result_tx.send(None);
                }
                return;
            }
            dg_stream::spawn_audio_bridge(recording_id, chunk_recv, session_tx);
        });
    } else {
        tracing::debug!("[dg_stream] no chunk receiver — WS streaming not started");
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

    if let Ok(mut route) = app.state::<RecordingRouteState>().0.lock() {
        *route = None;
    }

    let _ = app
        .state::<StreamingState>()
        .0
        .lock()
        .ok()
        .and_then(|mut g| g.take());

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

    sync_tray(&app, &snap);
    let _ = app.emit("app-state", &snap);
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
    LAST_FINISH_MS.store(now_ms_desktop(), Ordering::SeqCst);
    reset_long_dictation_lock(&app);

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
    let begin_stop = {
        let mut d = match shared.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        match d.begin_stop() {
            Ok((stop_rx, was_too_short)) => {
                let snap = d.snapshot();
                tracing::info!("[record] stop initiated — state={}", snap.state);
                Ok((stop_rx, was_too_short, snap))
            }
            Err(e) => {
                if is_short_recording_cancel(&e) {
                    tracing::info!("[record] short Option tap — cancelled recording");
                    let snap = d.finish_cancelled();
                    Err(BeginStopError::Short(snap))
                } else {
                    let snap = d.finish_err(e);
                    Err(BeginStopError::Failed(snap))
                }
            }
        }
    };

    let (stop_rx, was_too_short) = match begin_stop {
        Ok((stop_rx, was_too_short, snap)) => {
            sync_tray(&app, &snap);
            let _ = app.emit("app-state", &snap);
            (stop_rx, was_too_short)
        }
        Err(BeginStopError::Short(snap)) => {
            sync_tray(&app, &snap);
            emit_short_recording_error(&app);
            let _ = app.emit("app-state", &snap);
            return;
        }
        Err(BeginStopError::Failed(snap)) => {
            sync_tray(&app, &snap);
            let _ = app.emit(
                "voice-error",
                serde_json::json!({
                    "message": snap.last_error.clone().unwrap_or_else(|| "Recording failed".to_string()),
                    "audio_id": null,
                }),
            );
            let _ = app.emit("app-state", &snap);
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
            let _ = app.emit("app-state", &snap);
            emit_meeting_stt_status(&app);
            return;
        }
    };

    // ── P5: Take the transcript receiver before spawning the async task ────────
    // Use ok() so a poisoned mutex from a previous panic doesn't cascade-crash.
    let transcript_rx = app
        .state::<StreamingState>()
        .0
        .lock()
        .ok()
        .and_then(|mut g| g.take());

    // Do the async SSE pipeline in a tokio task
    let shared2 = Arc::clone(&shared);
    let app2 = app.clone();
    let back_arc2 = Arc::clone(&back_arc);

    tauri::async_runtime::spawn(async move {
        // ── P5: Wait briefly for the Deepgram WS transcript ───────────────────
        // begin_stop() dropped chunk_tx, which makes the audio bridge send a
        // Deepgram Finalize command. The persistent WS actor usually returns
        // quickly; if it is disconnected or reconnecting it sends None so the
        // backend batch-STT fallback can take over without a long local wait.
        // Estimate recording duration from WAV size:
        // 16kHz × 16-bit × mono = 32,000 bytes/sec, plus 44 byte WAV header
        let wav_duration_s = (wav.len().saturating_sub(44)) as f64 / 32_000.0;

        let pre_transcript: Option<dg_stream::StreamingTranscript> = if let Some(rx) = transcript_rx
        {
            let wait_start = tokio::time::Instant::now();
            match tokio::time::timeout(std::time::Duration::from_millis(2500), rx).await {
                Ok(Ok(Some(t))) if !t.transcript.is_empty() => {
                    let wait_ms = wait_start.elapsed().as_millis();
                    // Quality gate: reject suspiciously short transcripts.
                    // Normal speech is 2–3 words/sec (120–180 WPM).
                    // Require at least 1 word/sec (60 WPM) — anything below
                    // that means the WS likely dropped segments during drain.
                    let word_count = if t.meta.word_count > 0 {
                        t.meta.word_count
                    } else {
                        t.transcript.split_whitespace().count()
                    };
                    let expected_min_words = wav_duration_s.max(1.0) as usize;
                    if word_count < expected_min_words && wav_duration_s > 3.0 {
                        tracing::warn!(
                            "[finish] WS transcript too short after {wait_ms}ms: {} words for {:.1}s recording (expected ≥{}) — falling back to HTTP STT. transcript={:?}",
                            word_count,
                            wav_duration_s,
                            expected_min_words,
                            t.transcript
                        );
                        None
                    } else {
                        tracing::info!(
                            "[finish] ✓ WS pre-transcript ready after {wait_ms}ms ({} chars, {} words, {:.1}s audio): \"{}\"",
                            t.transcript.len(),
                            word_count,
                            wav_duration_s,
                            t.transcript
                        );
                        Some(t)
                    }
                }
                Ok(Ok(Some(_))) => {
                    tracing::info!(
                        "[finish] WS transcript empty after {}ms — falling back to HTTP STT",
                        wait_start.elapsed().as_millis()
                    );
                    None
                }
                Ok(Ok(None)) => {
                    tracing::info!(
                        "[finish] WS unavailable after {}ms — falling back to HTTP STT",
                        wait_start.elapsed().as_millis()
                    );
                    None
                }
                Ok(Err(_)) => {
                    tracing::info!(
                        "[finish] WS transcript sender dropped after {}ms — falling back to HTTP STT",
                        wait_start.elapsed().as_millis()
                    );
                    None
                }
                Err(_) => {
                    tracing::warn!(
                        "[finish] WS transcript timed out after {}ms — falling back to HTTP STT",
                        wait_start.elapsed().as_millis()
                    );
                    None
                }
            }
        } else {
            None
        };

        let screen_context = app2
            .try_state::<ScreenContextState>()
            .and_then(|s| s.0.lock().ok()?.clone());

        let result = run_voice_polish_sse(
            &back_arc2,
            wav,
            None,
            pre_transcript,
            None,
            screen_context,
            &app2,
            is_meeting,
        )
        .await;

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
                    Err(ref e) => (d.finish_err(e.clone()), Some(e.clone())),
                }
            };
            if let Some(e) = err_msg {
                emit_voice_error_quiet(&app2, &e);
            }
            sync_tray(&app2, &snap);
            let _ = app2.emit("app-state", &snap);
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
                start_edit_watcher(
                    back3,
                    app2.clone(),
                    done.recording_id.clone(),
                    done.polished.clone(),
                    watch_start,
                    edit_target_pid,
                    pre_paste,
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
                    Err(ref e) => (d.finish_err(e.clone()), Some(e.clone())),
                }
            };
            if let Some(e) = err_msg {
                emit_voice_error_quiet(&app2, &e);
            }
            sync_tray(&app2, &snap);
            let _ = app2.emit("app-state", &snap);
            emit_meeting_stt_status(&app2);
        }
    });
}

/// Async SSE consumer: streams tokens from backend, types them word-by-word,
/// and stores the result for Ctrl+Cmd+V re-paste.
/// In meeting mode (`is_meeting`), skips word-by-word typing entirely.
async fn run_voice_polish_sse(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    wav: Vec<u8>,
    target_app: Option<String>,
    pre_transcript: Option<dg_stream::StreamingTranscript>,
    repair_mode: Option<String>,
    screen_context: Option<String>,
    app: &tauri::AppHandle,
    #[allow(unused_variables)] is_meeting: bool,
) -> Result<api::PolishDone, String> {
    let ep = {
        let lock = back_arc.lock().map_err(|_| "backend lock failed")?;
        lock.clone().ok_or("backend not started")?
    };

    let app_clone = app.clone();

    // Track whether word-by-word AX typing succeeded
    let typed_any = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let typed_any2 = typed_any.clone();
    let token_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let token_count2 = token_count.clone();
    let fail_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count2 = fail_count.clone();
    let live_guard = std::sync::Arc::new(std::sync::Mutex::new(LiveTypingGuard::default()));
    let live_guard2 = live_guard.clone();
    let typed_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let typed_text2 = typed_text.clone();
    let initial_field_text = if is_meeting {
        None
    } else {
        paster::read_focused_value_fast()
    };

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
            .unwrap_or_else(|| "none (will use HTTP STT)".into()),
    );

    let mut on_polish_event = move |event| {
        match &event {
            api::PolishEvent::Token { token } => {
                // In meeting mode, skip all typing — only emit for live preview
                if is_meeting {
                    let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
                    return;
                }
                let decision = live_guard2
                    .lock()
                    .map(|mut guard| guard.on_token(token))
                    .unwrap_or(LiveTypingDecision::PreviewOnly);
                // Emit to UI for live preview
                let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
                match decision {
                    LiveTypingDecision::ResetAndDisable => {
                        tracing::warn!(
                            "[main] live typing reset — disabling word-by-word for this recording, will paste full output at end"
                        );
                        fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    LiveTypingDecision::PreviewOnly => {
                        return;
                    }
                    LiveTypingDecision::TypeToken => {}
                }
                // Type word-by-word directly into focused app via AX
                match paster::type_text(token) {
                    Ok(true) => {
                        if let Ok(mut text) = typed_text2.lock() {
                            text.push_str(token);
                        }
                        let prev = typed_any2.swap(true, std::sync::atomic::Ordering::Relaxed);
                        let n = token_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if !prev {
                            tracing::info!(
                                "[main] GAP-2: word-by-word typing started — first token {:?}",
                                token
                            );
                        }
                        let _ = n;
                    }
                    Ok(false) => {
                        fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    Err(e) => {
                        fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        tracing::warn!("[main] type_text error: {e}");
                    }
                }
            }
            api::PolishEvent::Status { phase, transcript } => {
                tracing::info!(
                    "[pipeline] status: phase={phase} transcript={}",
                    transcript.as_deref().unwrap_or("—")
                );
                let _ = app_clone.emit(
                    "voice-status",
                    serde_json::json!({ "phase": phase, "transcript": transcript }),
                );
            }
            api::PolishEvent::Done(done) => {
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
                let _ = app_clone.emit("voice-done", done);
            }
            api::PolishEvent::Error {
                message,
                audio_id,
                error_code,
            } => {
                tracing::error!("[pipeline] backend error: {message}");
                let human = humanize_error(&message);
                let _ = app_clone.emit(
                    "voice-error",
                    serde_json::json!({
                        "message":  human,
                        "raw_error": message,
                        "audio_id": audio_id,
                        "error_code": error_code,
                        "auto_hide_ms": 4000,
                    }),
                );
            }
        }
    };

    let done = if let Some(transcript) = pre_transcript {
        tracing::info!("[pipeline] fast path: sending WAV + WS transcript to backend");
        api::stream_voice_polish(
            &ep,
            wav,
            target_app,
            Some(transcript.transcript),
            Some(transcript.meta),
            repair_mode,
            screen_context,
            &mut on_polish_event,
        )
        .await?
    } else {
        api::stream_voice_polish(
            &ep,
            wav,
            target_app,
            None,
            None,
            repair_mode,
            screen_context,
            &mut on_polish_event,
        )
        .await?
    };

    let n_typed = token_count.load(std::sync::atomic::Ordering::Relaxed);
    let n_failed = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let mut output_pasted = false;
    if is_meeting {
        tracing::info!("[main] meeting mode — skipping paste for polished chunk");
    } else if typed_any.load(std::sync::atomic::Ordering::Relaxed) {
        let typed_snapshot = typed_text
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        if n_failed > 0 {
            // Some tokens typed, then the stream reset or a token failed. Reconcile
            // only the text AirNote typed for this recording; never Cmd+A the field.
            tracing::warn!(
                "[main] word-by-word partial: {n_typed} ok, {n_failed} failed — reconciling current typed text"
            );
            if !done.polished.is_empty() {
                match paster::reconcile_current_recording(
                    initial_field_text.as_deref(),
                    &typed_snapshot,
                    &done.polished,
                ) {
                    Ok(changed) => {
                        if changed {
                            tracing::info!(
                                "[main] current typed text reconciled after partial stream"
                            );
                        }
                        output_pasted = true;
                    }
                    Err(e) => {
                        tracing::warn!("[main] typed-text reconciliation failed: {e}");
                    }
                }
            }
        } else if done.polished.is_empty() {
            tracing::warn!(
                "[main] word-by-word complete but final output is empty — keeping streamed text"
            );
            output_pasted = !typed_snapshot.is_empty();
        } else if typed_snapshot == done.polished {
            tracing::info!("[main] word-by-word complete — {n_typed} token(s) typed directly");
            output_pasted = true;
        } else {
            tracing::info!(
                "[main] word-by-word complete — final text differs, reconciling current typed text (typed_chars={} final_chars={})",
                typed_snapshot.chars().count(),
                done.polished.chars().count()
            );
            match paster::reconcile_current_recording(
                initial_field_text.as_deref(),
                &typed_snapshot,
                &done.polished,
            ) {
                Ok(changed) => {
                    if changed {
                        tracing::info!("[main] current typed text reconciled with final output");
                    }
                    output_pasted = true;
                }
                Err(e) => {
                    tracing::warn!("[main] final typed-text reconciliation failed: {e}");
                    output_pasted = !typed_snapshot.is_empty();
                }
            }
        }
    } else {
        // Live token typing did not produce output. Insert the final result
        // directly first so normal dictation does not touch the user's
        // clipboard; fall back to Cmd+V only if direct typing fails.
        tracing::info!(
            "[main] live typing produced no output — direct insert final result ({} chars)",
            done.polished.len()
        );
        if !done.polished.is_empty() {
            match insert_text_prefer_direct("main_final_insert", &done.polished) {
                Ok(_) => {
                    output_pasted = true;
                }
                Err(e) => {
                    tracing::warn!("[main] final insert failed: {e}");
                }
            }
        }
    }

    // Always store latest result so Ctrl+Cmd+V can re-paste it any time
    if !done.polished.is_empty() {
        if let Ok(mut g) = app.state::<LatestResult>().0.lock() {
            *g = Some(done.polished.clone());
        }
        cache_last_voice_action(app, &done, LastRepairStage::None);
        tracing::debug!(
            "[main] result stored ({} chars) — Ctrl+Cmd+V to paste again",
            done.polished.len()
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
        "Sent to meeting"
    } else if output_pasted {
        "Pasted"
    } else {
        // The Ctrl+Cmd+V "re-paste" hotkey is only wired on macOS today;
        // Windows users use the tray menu's "Paste latest" item instead.
        #[cfg(target_os = "macos")]
        {
            "Press Ctrl+Cmd+V to paste anywhere"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "Use the tray menu → Paste latest"
        }
    };
    tracing::debug!("[main] voice-output status={output_status}");
    let _ = app.emit(
        "voice-output",
        serde_json::json!({
            "status": output_status,
            "message": output_message,
        }),
    );

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
            let _ = app_clone.emit(
                "voice-status",
                serde_json::json!({ "phase": phase, "transcript": transcript }),
            );
        }
        api::PolishEvent::Done(done) => {
            let _ = app_clone.emit("voice-done", done);
        }
        api::PolishEvent::Error {
            message,
            audio_id,
            error_code,
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
            let _ = app_clone.emit(
                "voice-status",
                serde_json::json!({ "phase": phase, "transcript": transcript }),
            );
        }
        api::PolishEvent::Done(done) => {
            let _ = app_clone.emit("voice-done", done);
        }
        api::PolishEvent::Error {
            message,
            audio_id,
            error_code,
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
        "Repaired"
    } else {
        // The Ctrl+Cmd+V "re-paste" hotkey is only wired on macOS today;
        // Windows users use the tray menu's "Paste latest" item instead.
        #[cfg(target_os = "macos")]
        {
            "Repair ready — press Ctrl+Cmd+V to paste"
        }
        #[cfg(not(target_os = "macos"))]
        {
            "Repair ready — use Paste latest"
        }
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
/// Invoked by the Ctrl+Cmd+V hotkey and by the UI's "Paste latest" button.
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
            paster::paste(&t).map_err(|e| format!("paste failed: {e}"))?;
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
                Err(e) => d.finish_err(e),
            }
        };
        sync_tray(&app2, &snap);
        let _ = app2.emit("app-state", &snap);
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
        let _ = app2.emit("app-state", &snap);
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

/// Retry a failed recording by re-submitting its saved WAV file.
/// `audio_id` is the UUID that the backend included in the `voice-error` event.
#[tauri::command]
fn retry_recording(
    audio_id: String,
    state: State<'_, SharedApp>,
    backend: State<'_, BackendState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    retry_recording_spawn(audio_id, Arc::clone(&state.0), Arc::clone(&backend.0), app)
}

fn retry_recording_internal(app: tauri::AppHandle, audio_id: String) {
    let shared = Arc::clone(&app.state::<SharedApp>().0);
    let backend = Arc::clone(&app.state::<BackendState>().0);
    let _ = retry_recording_spawn(audio_id, shared, backend, app);
}

fn retry_recording_spawn(
    audio_id: String,
    shared: Arc<Mutex<DesktopApp>>,
    backend: Arc<Mutex<Option<BackendEndpoint>>>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Read WAV from the saved file
    let audio_dir = {
        let base = dirs::data_local_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        base.join("VoicePolish").join("audio")
    };
    let wav_path = audio_dir.join(format!("{audio_id}.wav"));
    let wav = std::fs::read(&wav_path).map_err(|e| format!("saved audio not found: {e}"))?;

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
        let result = run_voice_polish_sse(
            &back_arc2,
            wav,
            None,
            None,
            Some("preserve_recall".into()),
            None, // no screen context for re-polish
            &app2,
            false,
        )
        .await;

        let watch_start = std::time::Instant::now();
        if let Ok(ref done) = result {
            let back3 = Arc::clone(&back_arc2);
            start_edit_watcher(
                back3,
                app2.clone(),
                done.recording_id.clone(),
                done.polished.clone(),
                watch_start,
                None,
                None, // re-polish: no pre_paste context
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
        let _ = app2.emit("app-state", &snap);
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
    hot_cache: State<'_, HotPathCache>,
    dg_session: State<'_, DeepgramSessionState>,
    id: String,
    action: String,
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let result = api::resolve_pending_edit(&ep, &id, &action).await;
    // "approve" promotes a term into vocabulary — refresh cache immediately.
    if result.is_ok() && action == "approve" {
        let arc = Arc::clone(&hot_cache.0);
        let session_tx = dg_session.0.clone();
        let ep2 = ep.clone();
        tauri::async_runtime::spawn(async move {
            if let Ok(bias) = api::get_stt_bias(&ep2).await {
                tracing::info!(
                    "[hot_cache] refreshed after pending-edit approval (mode={} keyterms={} replacements={})",
                    bias.stt_mode,
                    bias.keyterms.len(),
                    bias.replacements.len()
                );
                let mut hot = arc.write().await;
                hot.stt_mode = bias.stt_mode;
                hot.keyterms = bias.keyterms;
                hot.replacements = bias.replacements;
                let deepgram_key = hot.deepgram_key.clone();
                let session_bias = said_core::deepgram::BiasPackage {
                    stt_mode: hot.stt_mode.clone(),
                    keyterms: hot.keyterms.clone(),
                    replacements: hot.replacements.clone(),
                };
                drop(hot);
                let _ = session_tx
                    .send(dg_stream::SessionCommand::Reconfigure {
                        deepgram_key,
                        bias: session_bias,
                    })
                    .await;
            }
        });
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
) -> Result<(), String> {
    let ep = get_endpoint(&backend)?;
    let body = serde_json::json!({
        "term": term,
        "original": original,
        "action": action,
        "recording_id": recording_id,
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
) -> Result<usize, String> {
    let ep = get_endpoint(&backend)?;
    let pairs: Vec<(String, String)> = items
        .iter()
        .filter_map(|v| {
            let orig = v.get("original")?.as_str()?.to_string();
            let corr = v.get("corrected")?.as_str()?.to_string();
            Some((orig, corr))
        })
        .collect();
    let result = api::confirm_batch(&ep, &pairs, recording_id.as_deref()).await?;
    let _ = app.emit("vocabulary-changed", ());
    tracing::info!(
        "[confirm-batch] user confirmed {} term(s): {:?}",
        result.learned_count,
        result.learned_terms,
    );
    Ok(result.learned_count)
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
    said_core::prefs::load()
}

#[tauri::command]
fn set_desktop_prefs(prefs: said_core::prefs::DesktopPrefs) -> Result<(), String> {
    said_core::prefs::save(&prefs)
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
    if let Some(idx) = text.rfind(marker) {
        text = text[idx..].to_string();
    }
    (text, start > 0)
}

#[tauri::command]
fn get_debug_logs() -> DebugLogs {
    let dir = said_log_dir();
    let desktop_path = dir.join("said.log");
    let backend_path = dir.join("backend.log");
    let (desktop, desktop_truncated) =
        read_recent_log(&desktop_path, "[main] said desktop starting");
    let (backend, backend_truncated) = read_recent_log(&backend_path, "airnote-backend build=");

    let combined = format!(
        "── AirNote desktop ({}) ──\n{}\n\n── airnote-backend ({}) ──\n{}",
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
    );

    DebugLogs {
        desktop_path: desktop_path.display().to_string(),
        backend_path: backend_path.display().to_string(),
        desktop,
        backend,
        combined,
        truncated: desktop_truncated || backend_truncated,
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
    sys.refresh_memory();
    sys.refresh_cpu_all();
    sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);

    let desktop_pid = sysinfo::Pid::from_u32(std::process::id());
    let desktop = sys
        .process(desktop_pid)
        .map(|process| process_perf(desktop_pid, process));

    let owned_backend_pid = backend_handle
        .0
        .lock()
        .ok()
        .and_then(|handle| handle.as_ref().and_then(|h| h.pid()))
        .map(sysinfo::Pid::from_u32);

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

/// Faster settle for single high-jargon word replacements.
/// If the user replaced exactly one word and it has brand/acronym
/// characteristics, fire classify after just 1.5 seconds of stability.
const EDIT_QUICK_SETTLE_MS: u64 = 1500;

/// Compute edit-watch timeouts scaled by sentence length.
/// Short sentences (≤15 words) = 15s max, 6s idle, 3s settle.
/// Long sentences (50+ words) = 45s max, 12s idle, 8s settle.
/// Users need more time to read and find errors in longer text.
fn edit_watch_timeouts(word_count: usize) -> (Duration, Duration, u64) {
    let words = word_count.max(5).min(80) as f64;
    // Linear scale: 15s base + 0.6s per word above 15
    let max_secs = 15.0 + (words - 15.0).max(0.0) * 0.6;
    let max_duration = Duration::from_secs_f64(max_secs.min(45.0));
    // Idle timeout: 6s base + 0.15s per word above 15
    let idle_secs = 6.0 + (words - 15.0).max(0.0) * 0.15;
    let idle_timeout = Duration::from_secs_f64(idle_secs.min(15.0));
    // Settle: 3s base + 0.1s per word above 15
    let settle_secs = (3.0 + (words - 15.0).max(0.0) * 0.1).min(10.0);
    (max_duration, idle_timeout, settle_secs as u64)
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
        prev.cancel();
    }
}

fn start_edit_watcher(
    back_arc: Arc<Mutex<Option<BackendEndpoint>>>,
    app: tauri::AppHandle,
    recording_id: String,
    polished: String,
    watch_start: std::time::Instant,
    target_pid: Option<i32>,
    pre_paste_text: Option<String>,
) {
    let token = {
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
            prev.cancel();
            std::thread::yield_now();
        }
        let token = CancellationToken::new();
        *guard = Some(token.clone());
        token
    };

    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        watch_for_edit(
            token.clone(),
            back_arc,
            app_for_task.clone(),
            recording_id,
            polished,
            watch_start,
            target_pid,
            pre_paste_text,
        )
        .await;

        if !token.is_cancelled() {
            if let Ok(mut guard) = app_for_task.state::<EditWatcherState>().0.lock() {
                *guard = None;
            }
        }
    });
}

/// After pasting, poll the focused text element for up to 30 seconds.
/// When the user stops typing for 5 s (or switches apps), emit "edit-detected"
/// so the frontend can ask "Save this preference?" before writing to SQLite.
async fn watch_for_edit(
    token: CancellationToken,
    back_arc: Arc<Mutex<Option<BackendEndpoint>>>,
    app: tauri::AppHandle,
    recording_id: String,
    polished: String,                // the AI-generated text we pasted
    watch_start: std::time::Instant, // captured at the call site, right after paste
    target_pid: Option<i32>,
    pre_paste_text: Option<String>, // field text BEFORE AirNote typed (from ScreenContextState)
) {
    use std::time::Instant;

    // Let the paste animation settle and focus move into the text field.
    // 400ms covers paste animation (~200ms) + AX cache start; retries handle the rest.
    if !cancellable_sleep(&token, Duration::from_millis(400)).await {
        tracing::info!("[edit-watch] watcher cancelled before start for {recording_id}");
        return;
    }

    // Prefer the PID captured when recording started. That is the key
    // OpenWhispr-style fix: we monitor the app the user dictated into, not
    // whatever happens to be frontmost after our HUD/status UI has updated.
    let focused_pid_after_paste =
        blocking_ax_option("focused_pid after-paste", paster::focused_pid).await;
    let initial_pid = target_pid.or(focused_pid_after_paste);

    // Attempt to get the initial field value.  Chrome / Electron may still be
    // building their AX cache even after the pre-unlock at recording-start, so
    // we retry a few times with increasing delays before declaring "AX blind".
    let post_paste = {
        let mut val =
            blocking_ax_option(
                "read_focused_value_first initial",
                move || match initial_pid {
                    Some(pid) => paster::read_focused_value_first_for_pid(pid),
                    None => paster::read_focused_value_first(),
                },
            )
            .await
            .unwrap_or_default();
        if val.is_empty() {
            // 2nd attempt after 300 ms
            if !cancellable_sleep(&token, Duration::from_millis(300)).await {
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
            if !cancellable_sleep(&token, Duration::from_millis(500)).await {
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

    let mut last_val = post_paste.clone();
    let mut best_candidate = post_paste.clone();
    let mut idle_at = Instant::now();
    let started = Instant::now();
    let mut last_change_at = Instant::now();
    let mut current_interval = EDIT_WATCH_FAST_INTERVAL;
    let mut last_pid = initial_pid;

    // Scale timeouts by sentence length — long sentences need more reading time
    let word_count = polished.split_whitespace().count();
    let (max_duration, idle_timeout, stable_settle_secs) = edit_watch_timeouts(word_count);
    tracing::info!(
        "[edit-watch] word_count={word_count} max={}s idle={}s settle={stable_settle_secs}s",
        max_duration.as_secs(),
        idle_timeout.as_secs(),
    );
    let mut edit_stable_since: Option<Instant> = None;
    let mut last_edit_snapshot: Option<String> = None;
    // Capture-error metadata, hoisted so we can ship it to the backend's
    // CAPTURE_ERROR pre-filter alongside the edit text.
    let mut app_switched_during_capture: bool = false;

    tracing::info!(
        "[edit-watch] watching {recording_id} — target_pid={:?} focused_after_paste={:?} initial field readable: {} (len={})",
        target_pid,
        focused_pid_after_paste,
        !post_paste.is_empty(),
        post_paste.len(),
    );

    // Poll loop: adaptive cadence.  No mid-loop side effects — we only read AX
    // and watch for app switches.  Clipboard verification (Cmd+A+C) is
    // strictly an end-of-loop, same-app operation; doing it during the loop
    // disrupts the user's typing in AX-blind apps like Claude input.
    loop {
        if !cancellable_sleep(&token, current_interval).await {
            tracing::info!("[edit-watch] watcher cancelled for {recording_id}");
            return;
        }

        // Check the frontmost PID first. With a locked target PID we keep
        // monitoring that original app even if focus moves; without one, keep
        // the old safety behavior and stop before reading another app's field.
        let now_pid = blocking_ax_option("focused_pid poll", paster::focused_pid).await;
        let pid_switched = matches!(
            (initial_pid, now_pid),
            (Some(a), Some(b)) if a != b
        );
        if pid_switched && target_pid.is_none() {
            app_switched_during_capture = true;
            tracing::info!(
                "[edit-watch] app_switched_skip for {recording_id} — initial_pid={initial_pid:?} now_pid={now_pid:?}"
            );
            break;
        } else if pid_switched {
            app_switched_during_capture = true;
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
            idle_at = Instant::now();
            last_change_at = Instant::now();
            current_interval = EDIT_WATCH_FAST_INTERVAL;
            // Only promote to best_candidate if the value still shares words
            // with the polished text (guards against Send-cleared placeholders).
            if shares_word_overlap(&now_val, &polished) {
                best_candidate = now_val.clone();
            }
            last_edit_snapshot = Some(now_val.clone());
            edit_stable_since = None; // edit is active, not stable yet
            last_val = now_val;
        } else if last_change_at.elapsed() > Duration::from_secs(2) {
            current_interval = EDIT_WATCH_SLOW_INTERVAL;
            // Track stable-edit: field stopped changing after an edit was seen
            if last_edit_snapshot.is_some() && edit_stable_since.is_none() {
                edit_stable_since = Some(Instant::now());
            }
        }

        // NEW: stable edit detection — fire early if edit region stopped changing
        if let Some(stable_since) = edit_stable_since {
            if stable_since.elapsed().as_secs() >= stable_settle_secs {
                tracing::info!(
                    "[edit-watch] edit stabilised for {stable_settle_secs}s — firing classify (total {}ms, words={word_count})",
                    started.elapsed().as_millis(),
                );
                break;
            }
        }

        let done = idle_at.elapsed() > idle_timeout || started.elapsed() > max_duration;

        if done {
            break;
        }
    }

    // If the final field value lost all overlap with our polished text (e.g. the
    // user sent the message and the input reverted to a placeholder), use the last
    // meaningful intermediate value instead.
    let effective_val = if shares_word_overlap(&last_val, &polished) {
        last_val.clone()
    } else if best_candidate != post_paste {
        tracing::info!(
            "[edit-watch] last_val lost overlap with polished (sent message?); using best_candidate"
        );
        best_candidate.clone()
    } else {
        last_val.clone()
    };

    let final_front_pid = blocking_ax_option("focused_pid final", paster::focused_pid).await;
    tracing::info!(
        "[edit-watch] done watching {recording_id} — field changed: {}, target still frontmost: {}",
        effective_val != post_paste,
        matches!((initial_pid, final_front_pid), (Some(a), Some(b)) if a == b),
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
            return;
        }
        user_kept = extract_kept(
            &polished,
            &post_paste,
            &effective_val,
            pre_paste_text.as_deref(),
        );
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
        return;
    }

    // ── Pre-flight gates (cheap, no API call) ─────────────────────────────────

    if user_kept.is_empty() || user_kept.trim() == polished.trim() {
        tracing::info!("[edit-watch] no diff for {recording_id} — skipping");
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
        return;
    }

    // Whitespace / punctuation / AX-jitter filter (no API call needed).
    if !is_meaningful_edit(&polished, &user_kept) {
        tracing::info!("[edit-watch] edit not meaningful for {recording_id} — skipping");
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
        let capture_meta = api::CaptureMeta {
            time_since_paste_ms: watch_start.elapsed().as_millis() as u64,
            app_switched: app_switched_during_capture,
            matches_clipboard: false,
        };
        match api::classify_edit(
            ep,
            &recording_id,
            &polished,
            &user_kept,
            capture_method,
            capture_meta,
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

                if let Some(email) = resp.learned_emails.first() {
                    if !email.trim().is_empty() {
                        if let Some(w) = app.get_webview_window("status-bar") {
                            let _ = w.show();
                        }
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
                    if let Some(w) = app.get_webview_window("status-bar") {
                        let _ = w.show();
                    }
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
                        if let Some(w) = app.get_webview_window("status-bar") {
                            let _ = w.show();
                        }
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
                    if let Some(w) = app.get_webview_window("status-bar") {
                        let _ = w.show();
                    }
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
                            })
                        })
                        .collect();
                    let _ = app.emit(
                        "vocab-review",
                        serde_json::json!({
                            "candidates": candidates,
                            "recording_id": recording_id,
                        }),
                    );
                    tracing::info!(
                        "[edit-watch] review card: {} candidate(s)",
                        resp.review_candidates.len(),
                    );
                }

                // Ambiguous terms — show confirmation toast in status bar
                for amb in &resp.ambiguous_terms {
                    if let Some(w) = app.get_webview_window("status-bar") {
                        let _ = w.show();
                    }
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
                    if let Some(w) = app.get_webview_window("status-bar") {
                        let _ = w.show();
                    }
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
                                if let Some(w) = app_retrain.get_webview_window("status-bar") {
                                    let _ = w.show();
                                }
                                let _ = app_retrain.emit(
                                    "retrain-status",
                                    serde_json::json!({ "phase": "started" }),
                                );
                                tracing::info!("[retrain-poll] training started — notified UI");
                            }

                            if status.finished_at > baseline_finished {
                                let dur = status.duration_ms as f64 / 1000.0;
                                if let Some(w) = app_retrain.get_webview_window("status-bar") {
                                    let _ = w.show();
                                }
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
                    });
                }

                // If vocabulary was updated, refresh the hot-path cache now so
                // the very next recording already uses the newly learned terms.
                if resp.learned && resp.promoted_count > 0 {
                    let hot_arc = Arc::clone(&app.state::<HotPathCache>().0);
                    let session_tx = app.state::<DeepgramSessionState>().0.clone();
                    let ep2 = ep.clone();
                    tokio::spawn(async move {
                        if let Ok(bias) = api::get_stt_bias(&ep2).await {
                            tracing::info!(
                                "[hot_cache] refreshed after learning — mode={} keyterms={} replacements={}",
                                bias.stt_mode,
                                bias.keyterms.len(),
                                bias.replacements.len()
                            );
                            let mut hot = hot_arc.write().await;
                            hot.stt_mode = bias.stt_mode;
                            hot.keyterms = bias.keyterms;
                            hot.replacements = bias.replacements;
                            let deepgram_key = hot.deepgram_key.clone();
                            let session_bias = said_core::deepgram::BiasPackage {
                                stt_mode: hot.stt_mode.clone(),
                                keyterms: hot.keyterms.clone(),
                                replacements: hot.replacements.clone(),
                            };
                            drop(hot);
                            let _ = session_tx
                                .send(dg_stream::SessionCommand::Reconfigure {
                                    deepgram_key,
                                    bias: session_bias,
                                })
                                .await;
                        }
                    });
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

/// Given what we pasted (`polished`), where the field was right after paste
/// (`post_paste`), the final field value (`last_val`), and optionally the field
/// text from BEFORE AirNote typed (`pre_paste`), extract only the user's edited
/// version of AirNote's output — stripping any pre-existing text.
fn extract_kept(
    polished: &str,
    post_paste: &str,
    last_val: &str,
    pre_paste: Option<&str>,
) -> String {
    // ── Strategy 1: use pre_paste to reliably find prefix/suffix ─────────
    // pre_paste = field content before AirNote typed.  post_paste = field content
    // after AirNote typed.  The common prefix between them is text before cursor;
    // the common suffix is text after cursor.  Whatever is in the middle of
    // post_paste is what AirNote actually inserted (after any app normalization).
    // We strip the same prefix/suffix from last_val to get the user's edit.
    if let Some(pre) = pre_paste {
        if !pre.is_empty() && post_paste.len() > pre.len() {
            let prefix_bytes = common_prefix_bytes(pre, post_paste);
            let pre_rest = &pre[prefix_bytes..];
            let post_rest = &post_paste[prefix_bytes..];
            let suffix_bytes = common_suffix_bytes(pre_rest, post_rest);

            let prefix = &post_paste[..prefix_bytes];
            let suffix = if suffix_bytes > 0 && suffix_bytes <= pre_rest.len() {
                &pre_rest[pre_rest.len() - suffix_bytes..]
            } else {
                ""
            };

            if last_val.starts_with(prefix) {
                let after_prefix = &last_val[prefix.len()..];
                if !suffix.is_empty() {
                    if let Some(middle) = after_prefix.strip_suffix(suffix) {
                        tracing::info!(
                            "[edit-watch] extract_kept via pre_paste: prefix={}b suffix={}b",
                            prefix_bytes,
                            suffix_bytes,
                        );
                        return middle.trim().to_string();
                    }
                }
                tracing::info!(
                    "[edit-watch] extract_kept via pre_paste prefix only: {}b",
                    prefix_bytes,
                );
                return after_prefix.trim().to_string();
            }
        }
    }

    // ── Strategy 2 (fallback): find polished verbatim in post_paste ──────
    let Some(offset) = post_paste.find(polished.trim()) else {
        return last_val.to_string();
    };

    let prefix = &post_paste[..offset];
    let after_end = offset + polished.trim().len();
    let suffix = &post_paste[after_end..];

    if let Some(lv_after_prefix) = last_val.strip_prefix(prefix) {
        if let Some(edited) = lv_after_prefix.strip_suffix(suffix) {
            return edited.trim().to_string();
        }
        return lv_after_prefix.trim().to_string();
    }

    last_val.to_string()
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
        .rfind(|c: char| c.is_alphanumeric())
        .map(|i| i + word[i..].chars().next().unwrap().len_utf8())
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
    install_rustls_crypto_provider();

    // 1. Load env vars from .env files
    said_core::load_env();

    // 1a. Sentry telemetry — must init before tracing so its panic hook
    //     stacks correctly. Held until main returns.
    let _sentry_guard = said_core::telemetry::init("said-desktop");

    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        backend_guard::kill_from_pid_file();
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
        .expect("cannot open said.log");
    // Three tracing layers: log file (always) + stderr (for `cargo run`
    // visibility) + Sentry (forwards ERROR events only).
    {
        use tracing_subscriber::fmt;
        use tracing_subscriber::prelude::*;

        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,said_hotkey=debug,said_paster=debug".into());

        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(log_file));

        let stderr_layer = fmt::layer().with_ansi(true).with_writer(std::io::stderr);

        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stderr_layer)
            .with(said_core::telemetry::tracing_layer())
            .init();
    }
    tracing::info!(
        "[main] said desktop starting — log file: {}",
        log_path.display()
    );

    // 3. Shared state
    let shared_app = Arc::new(Mutex::new(DesktopApp::new()));
    let backend_arc = Arc::new(Mutex::new(None::<BackendEndpoint>));

    let builder = tauri::Builder::default();
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());

    builder
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_enterprise_oauth_urls(app, &argv);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup({
            let shared   = Arc::clone(&shared_app);
            let back_arc = Arc::clone(&backend_arc);
            move |app| {
                #[cfg(target_os = "macos")]
                {
                    // Keep the app foreground/regular so the Dock icon remains
                    // visible. The recorder pill is a separate non-activating
                    // NSPanel, so we no longer switch the whole app to Accessory.
                    app.set_activation_policy(tauri::ActivationPolicy::Regular);
                    tracing::info!("[main] macOS activation policy set to Regular (dock visible)");
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
                        let ep2 = handle.endpoint();
                        if let Some(pid) = handle.pid() {
                            backend_guard::write_pid_file(pid);
                        }
                        *back_arc.lock().unwrap() = Some(ep.clone());
                        // Store the full handle so Drop kills the child on app exit.
                        // Without this the child outlives the app (zombie leak).
                        if let Ok(mut h) = app.state::<BackendHandleState>().0.lock() {
                            *h = Some(handle);
                        }
                        tracing::info!("[main] backend daemon ready");
                        // Seed the tray cache with real prefs so the first tray
                        // menu already shows the correct model checkmark.
                        let app_h = app.handle().clone();
                        tauri::async_runtime::spawn(async move {
                            // Fetch prefs + STT bias in parallel — both needed at startup.
                            let (prefs_res, stt_bias_res) = tokio::join!(
                                api::get_preferences(&ep),
                                api::get_stt_bias(&ep),
                            );
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
                            let deepgram_key = prefs_res
                                .as_ref()
                                .ok()
                                .and_then(|p| p.deepgram_api_key.clone())
                                .unwrap_or_default();
                            let stt_bias = stt_bias_res.unwrap_or_default();
                            tracing::info!(
                                "[hot_cache] seeded mode={} keyterms={} replacements={}",
                                stt_bias.stt_mode,
                                stt_bias.keyterms.len(),
                                stt_bias.replacements.len()
                            );
                            let hot = app_h.state::<HotPathCache>();
                            let mut c = hot.0.write().await;
                            c.language = language;
                            c.deepgram_key = deepgram_key.clone();
                            c.stt_mode = stt_bias.stt_mode.clone();
                            c.keyterms = stt_bias.keyterms.clone();
                            c.replacements = stt_bias.replacements.clone();

                            // Configure the persistent Deepgram session actor.
                            let bias = said_core::deepgram::BiasPackage {
                                stt_mode: c.stt_mode.clone(),
                                keyterms: c.keyterms.clone(),
                                replacements: c.replacements.clone(),
                            };
                            drop(c);
                            let dg_session = app_h.state::<DeepgramSessionState>();
                            let _ = dg_session
                                .0
                                .send(dg_stream::SessionCommand::Reconfigure {
                                    deepgram_key,
                                    bias,
                                })
                                .await;
                        });

                        // ── Periodic cache refresh every 5 minutes ────────────
                        // Belt-and-suspenders: even if an event-driven refresh
                        // fails (network blip, task panic), the cache catches up
                        // within 5 minutes.
                        let app_h = app.handle().clone();
                        tauri::async_runtime::spawn(async move {
                            let mut interval = tokio::time::interval(
                                std::time::Duration::from_secs(300)
                            );
                            interval.tick().await; // skip the immediate first tick
                            loop {
                                interval.tick().await;
                                match api::get_stt_bias(&ep2).await {
                                    Ok(bias) => {
                                        tracing::debug!(
                                            "[hot_cache] periodic refresh — mode={} keyterms={} replacements={}",
                                            bias.stt_mode,
                                            bias.keyterms.len(),
                                            bias.replacements.len()
                                        );
                                        let hot_state = app_h.state::<HotPathCache>();
                                        let mut hot = hot_state.0.write().await;
                                        hot.stt_mode = bias.stt_mode;
                                        hot.keyterms = bias.keyterms;
                                        hot.replacements = bias.replacements;
                                        let deepgram_key = hot.deepgram_key.clone();
                                        let session_bias = said_core::deepgram::BiasPackage {
                                            stt_mode: hot.stt_mode.clone(),
                                            keyterms: hot.keyterms.clone(),
                                            replacements: hot.replacements.clone(),
                                        };
                                        drop(hot);
                                        let dg_session =
                                            app_h.state::<DeepgramSessionState>();
                                        let _ = dg_session
                                            .0
                                            .send(dg_stream::SessionCommand::Reconfigure {
                                                deepgram_key,
                                                bias: session_bias,
                                            })
                                            .await;
                                    }
                                    Err(e) => {
                                        tracing::warn!("[hot_cache] periodic refresh failed: {e}");
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!("[main] failed to spawn backend: {e}");
                        // App continues without backend; commands return errors.
                    }
                }

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

                let tray_icon = tauri::image::Image::from_bytes(
                    include_bytes!("../icons/tray@2x.png")
                ).ok();

                let mut tray_builder = TrayIconBuilder::with_id("said")
                    .tooltip("AirNote — Voice Polish Studio")
                    .menu(&initial_menu)
                    .show_menu_on_left_click(true);

                if let Some(icon) = tray_icon {
                    tray_builder = tray_builder.icon(icon).icon_as_template(true);
                }

                tray_builder
                    .on_menu_event(|app, event| {
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
                    })
                    .build(app)?;

                // ── Close window → hide (keep running in menu bar) ────────────
                if let Some(window) = app.get_webview_window("main") {
                    let win = window.clone();
                    window.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            api.prevent_close();
                            let _ = win.hide();
                        }
                    });
                }

                // ── Floating status bar ────────────────────────────────────────
                create_status_bar(app.handle());

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
                                            "[hotkey] Fn pressed while long dictation locked → process"
                                        );
                                        long_locked.store(false, Ordering::SeqCst);
                                        long_stop_consumed.store(true, Ordering::SeqCst);
                                        do_finish_recording(shared, app_h, back);
                                    } else if current == Some(desktop::AppState::Idle) {
                                        do_start_recording(&shared, &app_h);
                                        if FINISH_AFTER_START.swap(false, Ordering::SeqCst) {
                                            tracing::info!(
                                                "[hotkey] release arrived during start — finishing immediately"
                                            );
                                            if hotkey_current_state(&shared, "finish after start")
                                                == Some(desktop::AppState::Recording)
                                            {
                                                do_finish_recording(shared, app_h, back);
                                            }
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
                                tracing::debug!("[hotkey] Fn release consumed after long dictation stop");
                            } else if long_locked_release.load(Ordering::SeqCst) {
                                tracing::info!(
                                    "[hotkey] Fn released while long dictation locked — keep listening"
                                );
                            } else {
                                // Normal AirNote route. This also applies while the
                                // meeting view is open but meeting capture is muted.
                                let back = Arc::clone(&back_hold_release);
                                std::thread::spawn(move || {
                                    let current = hotkey_current_state(&shared, "finish");
                                    if current == Some(desktop::AppState::Recording) {
                                        do_finish_recording(shared, app_h, back);
                                    } else if (current == Some(desktop::AppState::Idle)
                                        || current.is_none())
                                        && (HOTKEY_START_IN_FLIGHT.load(Ordering::SeqCst)
                                            || RECORDING_STARTING.load(Ordering::SeqCst))
                                    {
                                        tracing::info!(
                                            "[hotkey] release arrived before recording started — queue finish"
                                        );
                                        FINISH_AFTER_START.store(true, Ordering::SeqCst);
                                    }
                                });
                            }
                        }),
                    );

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
                                "[hotkey] Fn+Space ignored — meeting capture owns Fn"
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
                                    "[hotkey] Fn+Space lock pending until recording starts"
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
                        });
                    }));

                    // ── Option+1..5 tone shortcuts ─────────────────────────────
                    // Select text in any app, press Option+N to polish with a preset tone.
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
                                1 => tray_polish_message(&app_clone, "format"),
                                2 => tray_polish_message(&app_clone, "professional"),
                                3 => tray_polish_message(&app_clone, "casual"),
                                4 => tray_polish_message(&app_clone, "concise"),
                                5 => tray_polish_message(&app_clone, "hinglish"),
                                _ => {}
                            }
                        });
                    }));

                    // ── Ctrl+Cmd+V — paste latest stored result ─────────────────
                    let latest_arc = std::sync::Arc::clone(
                        &app.state::<LatestResult>().inner().0
                    );
                    hotkey::register_paste_callback(Arc::new(move || {
                        let text = {
                            let Ok(g) = latest_arc.lock() else { return };
                            g.clone()
                        };
                        if let Some(t) = text {
                            tracing::info!("[paste_hotkey] Ctrl+Cmd+V → pasting {} chars", t.len());
                            std::thread::spawn(move || {
                                if let Err(e) = paster::paste(&t) {
                                    tracing::warn!("[paste_hotkey] paste failed: {e}");
                                }
                            });
                        } else {
                            tracing::info!("[paste_hotkey] Ctrl+Cmd+V pressed but nothing stored yet");
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
        .manage(StreamingState(Mutex::new(None)))
        .manage(RecordingRouteState(Mutex::new(None)))
        .manage(DeepgramSessionState(dg_stream::DeepgramSession::spawn()))
        .manage(PerformanceState(Mutex::new(sysinfo::System::new_all())))
        .manage(TrayCache(Mutex::new(TrayCacheInner::default())))
        .manage(LatestResult(std::sync::Arc::new(Mutex::new(None))))
        .manage(LastActionState(Mutex::new(None)))
        .manage(HotPathCache(Arc::new(tokio::sync::RwLock::new(HotPathCacheInner::default()))))
        .manage(StatusBarHideGen(Arc::new(AtomicU64::new(0))))
        .manage(StatusBarPlacementActive(AtomicBool::new(false)))
        .manage(MeetingModeState::new())
        .manage(LongDictationState::new())
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            get_snapshot,
            dismiss_status_bar,
            resize_status_bar,
            get_status_bar_position,
            set_status_bar_position,
            reset_status_bar_position,
            set_status_bar_interactive,
            get_backend_endpoint,
            get_preferences,
            get_voice_prompt,
            save_voice_prompt_draft,
            apply_voice_prompt_draft,
            reset_voice_prompt,
            test_voice_prompt,
            patch_preferences,
            get_history,
            submit_edit_feedback,
            toggle_recording,
            set_mode,
            request_accessibility,
            request_input_monitoring,
            request_microphone,
            diagnose_ax,
            // Cloud auth
            cloud_signup,
            store_enterprise_auth,
            start_enterprise_oauth_listener,
            stop_enterprise_oauth_listener,
            clear_enterprise_auth,
            get_enterprise_status,
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
            reveal_downloaded_file,
            // Pending-edit review
            get_pending_edits,
            resolve_pending_edit,
            dismiss_pending_edit,
            // Vocabulary management
            list_vocabulary,
            add_vocabulary_term,
            delete_vocabulary_term,
            confirm_term,
            confirm_batch,
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
            // Meeting audio pipeline
            start_meeting_stt,
            stop_meeting_stt,
            toggle_meeting_mute,
            get_meeting_stt_status,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build AirNote desktop")
        .run(|app, event| match event {
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { has_visible_windows, .. } if !has_visible_windows => {
                show_main_window(app);
            }
            tauri::RunEvent::ExitRequested { code, api, .. } if code.is_none() => {
                // Window closed / Cmd+Q — hide instead of quit for accessory-app UX.
                api.prevent_exit();
            }
            tauri::RunEvent::Exit => {
                if let Ok(mut guard) = app.state::<BackendHandleState>().0.lock() {
                    drop(guard.take());
                }
                backend_guard::clear_pid_file();
            }
            _ => {}
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
mod live_typing_guard_tests {
    use super::{LiveTypingDecision, LiveTypingGuard, STREAM_RESET_SENTINEL};

    #[test]
    fn streams_until_reset_then_previews() {
        let mut guard = LiveTypingGuard::default();
        assert_eq!(guard.on_token("Hello"), LiveTypingDecision::TypeToken);
        assert_eq!(guard.on_token("world"), LiveTypingDecision::TypeToken);
        assert_eq!(
            guard.on_token(STREAM_RESET_SENTINEL),
            LiveTypingDecision::ResetAndDisable
        );
        assert_eq!(guard.on_token("final"), LiveTypingDecision::PreviewOnly);
    }
}
