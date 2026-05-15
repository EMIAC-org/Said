#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod backend;
mod backend_guard;
mod desktop;
mod dg_stream; // P5: Deepgram WebSocket live streaming
mod permissions;

use std::io::{Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{
    Emitter, Manager, State,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::TrayIconBuilder,
};
use tokio_util::sync::CancellationToken;

use backend::BackendEndpoint;
use desktop::DesktopApp;
use said_core::{AppSnapshot, ProcessSummary};
use said_paster as paster;

const DEBUG_LOG_MAX_BYTES: u64 = 240_000;
const STREAM_RESET_SENTINEL: &str = "\u{1F}__RESET__\u{1F}";

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

#[cfg(target_os = "macos")]
use said_hotkey as hotkey;

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
            ns_window.send_message(Sel::register("setIgnoresMouseEvents:"), (false,));
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

#[cfg(target_os = "macos")]
fn schedule_status_bar_macos_tune(win: &tauri::WebviewWindow) {
    let win_for_main = win.clone();
    if let Err(e) = win.run_on_main_thread(move || {
        configure_status_bar_macos(&win_for_main);
    }) {
        tracing::warn!("[status-bar] could not schedule macOS tune on main thread: {e}");
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
///    plugin posts via the app's own bundle, so the banner shows the Said
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
                tracing::info!("[notify] plugin sent (Said icon): {title}");
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

#[cfg(not(target_os = "macos"))]
fn notify_macos(_app: &tauri::AppHandle, _title: &str, _body: &str) {}

/// Translate a raw pipeline error string into one short human sentence.
///
/// We never want to surface diagnostic text like "Deepgram error 400:
/// invalid audio format" or "empty transcript — nothing spoken?" to the
/// user — those read like log lines.  This function maps the common cases
/// to plain English and falls back to a generic apology for everything
/// else.  Keep wording calm and blame-free; no "Error:" prefix, no codes,
/// no emoji.
fn humanize_error(raw: &str) -> String {
    let lower = raw.to_lowercase();

    // STT-side cases
    if lower.contains("empty transcript") || lower.contains("nothing spoken") {
        return "We couldn't hear anything in that recording. Try again.".to_string();
    }
    if lower.contains("deepgram")
        && (lower.contains("401") || lower.contains("403") || lower.contains("unauthorized"))
    {
        return "Speech recognition is offline because the API key isn't accepted. Check your Deepgram key in Settings.".to_string();
    }
    if lower.contains("deepgram") && (lower.contains("429") || lower.contains("rate")) {
        return "Speech recognition is rate-limited right now. Give it a few seconds and try again.".to_string();
    }
    if lower.contains("deepgram") || lower.contains("stt") {
        return "Speech recognition couldn't process that recording. Try again in a moment."
            .to_string();
    }

    // LLM-side cases
    if lower.contains("openai")
        && (lower.contains("not connected") || lower.contains("401") || lower.contains("403"))
    {
        return "OpenAI isn't connected. Open Settings to sign in again.".to_string();
    }
    if lower.contains("groq") && (lower.contains("401") || lower.contains("403")) {
        return "Groq isn't accepting the API key. Check it in Settings.".to_string();
    }
    if lower.contains("gemini") && (lower.contains("401") || lower.contains("403")) {
        return "Gemini isn't accepting the API key. Check it in Settings.".to_string();
    }
    if lower.contains("rate") || lower.contains("429") {
        return "The polish service is rate-limited right now. Try again in a moment.".to_string();
    }
    if lower.contains("preferences not found") {
        return "Settings aren't loaded yet. Wait a moment and try again.".to_string();
    }
    if lower.contains("missing_api_keys") || lower.contains("api keys required") {
        return "API keys required — open Settings to add them.".to_string();
    }

    // Network / transport
    if lower.contains("timeout") || lower.contains("timed out") {
        return "The connection took too long. Check your internet and try again.".to_string();
    }
    if lower.contains("dns") || lower.contains("unreachable") || lower.contains("failed to connect")
    {
        return "Couldn't reach the network. Check your internet and try again.".to_string();
    }

    // Generic fallback — never expose raw error text.
    "Something went wrong with that recording. Please try again.".to_string()
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
            LiveTypingDecision::PreviewOnly
        } else {
            LiveTypingDecision::TypeToken
        }
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

#[cfg(target_os = "macos")]
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
        "processing" => " …",
        _ => "",
    }
}

/// Build the dynamic tray menu.
/// Re-run on every state change so recording label and language checkmarks stay in sync.
fn build_tray_menu(
    app: &tauri::AppHandle,
    snap: &AppSnapshot,
    custom_prompt: Option<&str>,
    output_language: &str,
) -> Result<Menu<tauri::Wry>, tauri::Error> {
    // ── 1. Toggle recording (state-aware label + enabled) ──────────────
    let toggle_label = match snap.state.as_str() {
        "recording" => "Stop recording",
        "processing" => "Processing…",
        _ => "Start recording",
    };
    let toggle_enabled = snap.state.as_str() != "processing";
    let toggle = MenuItem::with_id(
        app,
        "tray_toggle",
        toggle_label,
        toggle_enabled,
        None::<&str>,
    )?;

    // ── 2. Output language submenu ─────────────────────────────────────
    let mk_lang =
        |id: &str, label: &str, active: bool| -> Result<MenuItem<tauri::Wry>, tauri::Error> {
            let prefix = if active { "✓  " } else { "    " };
            MenuItem::with_id(app, id, format!("{prefix}{label}"), true, None::<&str>)
        };
    let lang_hinglish = mk_lang(
        "tray_lang_hinglish",
        "Hinglish → Hinglish",
        output_language == "hinglish",
    )?;
    let lang_english = mk_lang(
        "tray_lang_english",
        "Hindi → Polished English",
        output_language == "english",
    )?;
    let lang_hindi = mk_lang(
        "tray_lang_hindi",
        "Hindi → Hindi (Devanagari)",
        output_language == "hindi",
    )?;
    let lang_submenu = Submenu::with_items(
        app,
        "Output Language",
        true,
        &[
            &lang_hinglish as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
            &lang_english,
            &lang_hindi,
        ],
    )?;

    // ── 4. "Polish my message" submenu ─────────────────────────────────
    // Shortcut hints: Option+1..5 (global hotkeys registered in setup).
    let p_format = MenuItem::with_id(
        app,
        "tray_polish_format",
        "Format Selected Text  ⌥1",
        true,
        None::<&str>,
    )?;
    let p_repair = MenuItem::with_id(
        app,
        "tray_smart_repair",
        "Smart Repair Last",
        true,
        None::<&str>,
    )?;
    let p_prof = MenuItem::with_id(
        app,
        "tray_polish_professional",
        "Professional English  ⌥2",
        true,
        None::<&str>,
    )?;
    let p_casual = MenuItem::with_id(app, "tray_polish_casual", "Casual  ⌥3", true, None::<&str>)?;
    let p_concise = MenuItem::with_id(
        app,
        "tray_polish_concise",
        "Concise  ⌥4",
        true,
        None::<&str>,
    )?;
    let p_hinglish = MenuItem::with_id(
        app,
        "tray_polish_hinglish",
        "Hinglish  ⌥5",
        true,
        None::<&str>,
    )?;
    let p_assertive = MenuItem::with_id(
        app,
        "tray_polish_assertive",
        "Assertive",
        true,
        None::<&str>,
    )?;
    let p_neutral = MenuItem::with_id(app, "tray_polish_neutral", "Neutral", true, None::<&str>)?;

    let polish_refs: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = {
        let mut v: Vec<Box<dyn tauri::menu::IsMenuItem<tauri::Wry>>> = vec![
            Box::new(p_format),
            Box::new(p_prof),
            Box::new(p_casual),
            Box::new(p_concise),
            Box::new(p_hinglish),
            Box::new(p_repair),
            Box::new(p_assertive),
            Box::new(p_neutral),
        ];
        // Add "Custom  ⌥5" only when the user has set a custom prompt in Settings
        if custom_prompt.map(|s| !s.trim().is_empty()).unwrap_or(false) {
            let p_custom =
                MenuItem::with_id(app, "tray_polish_custom", "Custom", true, None::<&str>)?;
            v.push(Box::new(p_custom));
        }
        v
    };
    let polish_item_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> =
        polish_refs.iter().map(|b| b.as_ref()).collect();
    let polish_submenu = Submenu::with_items(app, "Polish my message", true, &polish_item_refs)?;

    // ── 4. Window actions + quit ────────────────────────────────────────
    let show_item = MenuItem::with_id(app, "show", "Open Said", true, None::<&str>)?;
    let settings_item = MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Said", true, None::<&str>)?;

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let sep4 = PredefinedMenuItem::separator(app)?;

    Menu::with_items(
        app,
        &[
            &toggle as &dyn tauri::menu::IsMenuItem<tauri::Wry>,
            &sep1,
            &lang_submenu,
            &sep2,
            &polish_submenu,
            &sep3,
            &show_item,
            &settings_item,
            &sep4,
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

    tracing::info!("[status-bar] sync state={state}");
    if state == "idle" {
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
                    Ok(_) => tracing::info!("[status-bar] hidden after idle"),
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

    match win.set_always_on_top(true) {
        Ok(_) => tracing::debug!("[status-bar] set_always_on_top ok"),
        Err(e) => tracing::warn!("[status-bar] set_always_on_top failed: {e}"),
    }
    match win.set_visible_on_all_workspaces(true) {
        Ok(_) => tracing::debug!("[status-bar] set_visible_on_all_workspaces ok"),
        Err(e) => tracing::warn!("[status-bar] set_visible_on_all_workspaces failed: {e}"),
    }
    match win.show() {
        Ok(_) => tracing::info!("[status-bar] show ok for state={state}"),
        Err(e) => tracing::warn!("[status-bar] show failed for state={state}: {e}"),
    }
    #[cfg(target_os = "macos")]
    schedule_status_bar_macos_tune(&win);
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
    let idle_w = 300.0;
    let idle_h = 142.0;
    let bottom_offset = 64.0;
    let (x, y) = if let Ok(Some(m)) = app.primary_monitor() {
        let sf = m.scale_factor();
        let sw = m.size().width as f64 / sf;
        let sh = m.size().height as f64 / sf;
        let mx = m.position().x as f64 / sf;
        let my = m.position().y as f64 / sf;
        (
            mx + sw / 2.0 - idle_w / 2.0,
            my + sh - idle_h - bottom_offset,
        )
    } else {
        (560.0, 860.0)
    };

    let url = "index.html?view=statusbar#statusbar";
    tracing::info!(
        "[status-bar] creating window url={url} x={x:.0} y={y:.0} size={idle_w:.0}x{idle_h:.0} visible=false"
    );

    match tauri::WebviewWindowBuilder::new(app, "status-bar", tauri::WebviewUrl::App(url.into()))
        .title("Said")
        .inner_size(idle_w, idle_h)
        .position(x, y)
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
        Ok(win) => {
            tracing::info!("[status-bar] window created label={}", win.label());
            match win.url() {
                Ok(url) => tracing::info!("[status-bar] resolved url={url}"),
                Err(e) => tracing::warn!("[status-bar] could not read window url: {e}"),
            }
            #[cfg(target_os = "macos")]
            schedule_status_bar_macos_tune(&win);
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

/// Polish the currently selected text using the given tone preset.
///
/// Flow: read selection → POST /v1/text/polish (SSE) with tone_override → paste result.
fn tray_polish_message(app: &tauri::AppHandle, tone: &str) {
    let backend = app.state::<BackendState>();
    let ep_opt = backend.0.lock().ok().and_then(|g| g.clone());
    let ep = match ep_opt {
        Some(e) => e,
        None => {
            tracing::warn!("[tray_polish] backend not ready");
            emit_tray_error(
                app,
                "Said backend is still starting. Try again in a moment.",
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
                // Paste the polished text (replaces selection)
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
                "Said backend is still starting. Language will update once Settings are available.",
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
    if let Some(win) = app.get_webview_window("status-bar") {
        win.hide()
            .map_err(|e| format!("hide status bar failed: {e}"))?;
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
            #[cfg(target_os = "macos")]
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

/// Start recording. Called when user presses Caps Lock (or taps the button).
fn do_start_recording(shared: &Arc<Mutex<DesktopApp>>, app: &tauri::AppHandle) {
    cancel_edit_watcher(app, "new recording");

    // Lock and pre-unlock the frontmost app's AX tree BEFORE recording begins.
    // Chrome / Electron need ~150-200 ms to build their accessibility cache after
    // AXEnhancedUserInterface / AXManualAccessibility is set.  By unlocking here
    // we give the browser the full dictation window (typically 2-10 s) to get
    // ready, so that post-paste edit detection can read AXValue reliably.
    #[cfg(target_os = "macos")]
    {
        let pid = paster::lock_frontmost_app_now();
        tracing::debug!("[record] locked frontmost app for edit-watch pid={pid:?}");
        if let Ok(mut target) = app.state::<EditTargetState>().0.lock() {
            *target = pid;
        }
    }

    let started = match shared.lock() {
        Ok(mut d) => d.start_recording(),
        Err(_) => return,
    };
    match started {
        Ok(snap) => {
            tracing::info!("[record] started — state={}", snap.state);
            sync_tray(app, &snap);
            let _ = app.emit("app-state", &snap);
        }
        Err(e) => {
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
    let level_recv = shared.lock().ok().and_then(|mut d| d.take_level_receiver());
    if let Some(level_recv) = level_recv {
        let app_levels = app.clone();
        std::thread::spawn(move || {
            let mut smoothed = 0.0f32;
            let mut last_emit = std::time::Instant::now()
                .checked_sub(std::time::Duration::from_millis(40))
                .unwrap_or_else(std::time::Instant::now);
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
            }
            let _ = app_levels.emit("voice-level", serde_json::json!({ "level": 0.0 }));
        });
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

/// Stop recording, ship WAV to backend via SSE, paste the result.
fn do_finish_recording(
    shared: Arc<Mutex<DesktopApp>>,
    app: tauri::AppHandle,
    back_arc: Arc<Mutex<Option<BackendEndpoint>>>,
) {
    let edit_target_pid = app
        .state::<EditTargetState>()
        .0
        .lock()
        .ok()
        .and_then(|mut target| target.take());

    // Signal the recorder to stop while holding the app mutex, then wait for
    // samples and encode WAV after releasing it.
    let (stop_rx, was_too_short) = {
        let mut d = match shared.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(t) = app.tray_by_id("said") {
            let _ = t.set_title(Some("[  …  ]  Said"));
        }
        match d.begin_stop() {
            Ok((stop_rx, was_too_short)) => {
                let snap = d.snapshot();
                tracing::info!("[record] stop initiated — state={}", snap.state);
                sync_tray(&app, &snap);
                let _ = app.emit("app-state", &snap);
                (stop_rx, was_too_short)
            }
            Err(e) => {
                if is_short_recording_cancel(&e) {
                    tracing::info!("[record] short Option tap — cancelled recording");
                    let snap = d.finish_cancelled();
                    sync_tray(&app, &snap);
                    emit_short_recording_error(&app);
                    let _ = app.emit("app-state", &snap);
                    return;
                }
                let snap = d.finish_err(e);
                sync_tray(&app, &snap);
                let _ = app.emit("voice-error", serde_json::json!({
                    "message": snap.last_error.clone().unwrap_or_else(|| "Recording failed".to_string()),
                    "audio_id": null,
                }));
                let _ = app.emit("app-state", &snap);
                return;
            }
        }
    };

    let wav = match desktop::DesktopApp::finish_stop(stop_rx, was_too_short) {
        Ok(wav) => wav,
        Err(e) => {
            let mut d = match shared.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            if is_short_recording_cancel(&e) {
                tracing::info!("[record] short Option tap — cancelled recording");
                let snap = d.finish_cancelled();
                sync_tray(&app, &snap);
                emit_short_recording_error(&app);
                let _ = app.emit("app-state", &snap);
                return;
            }
            let snap = d.finish_err(e);
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
                            "[finish] ✓ WS pre-transcript ready after {wait_ms}ms ({} chars, {} words, {:.1}s audio): {:?}",
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

        let result = run_voice_polish_sse(&back_arc2, wav, None, pre_transcript, None, &app2).await;

        // Spawn edit-watcher immediately after paste (non-blocking).
        // Capture watch_start NOW — before the spawn — so the ring
        // buffer timestamp filter doesn't miss early mouse clicks.
        let watch_start = std::time::Instant::now();
        if let Ok(ref done) = result {
            let back3 = Arc::clone(&back_arc2);
            start_edit_watcher(
                back3,
                app2.clone(),
                done.recording_id.clone(),
                done.polished.clone(),
                watch_start,
                edit_target_pid,
            );
        }

        let mut d = match shared2.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let snap = match result {
            Ok(done) => d.finish_ok(ProcessSummary {
                transcript: done.transcript.clone(),
                polished: done.polished,
                model: done.model_used,
                confidence: done.confidence.unwrap_or(0.0),
                transcribe_ms: done.latency_ms.transcribe as u64,
                polish_ms: done.latency_ms.polish as u64,
            }),
            Err(e) => d.finish_err(e),
        };
        sync_tray(&app2, &snap);
        let _ = app2.emit("app-state", &snap);
    });
}

/// Async SSE consumer: streams tokens from backend, types them word-by-word,
/// and stores the result for Ctrl+Cmd+V re-paste.
async fn run_voice_polish_sse(
    back_arc: &Arc<Mutex<Option<BackendEndpoint>>>,
    wav: Vec<u8>,
    target_app: Option<String>,
    pre_transcript: Option<dg_stream::StreamingTranscript>,
    repair_mode: Option<String>,
    app: &tauri::AppHandle,
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
                tracing::info!("[pipeline] status: phase={phase} transcript={transcript:?}");
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
                tracing::info!("[pipeline] polished text: {preview:?}{suffix}");
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
                        "message":  human.clone(),
                        "audio_id": audio_id,
                        "error_code": error_code,
                    }),
                );
                // Native macOS banner — informational only.  In dev mode the
                // osascript path can't attach action buttons (those require a
                // bundled .app with a registered UNNotificationCategory), so
                // the banner simply tells the user something went wrong and
                // they switch to Said when ready.  The in-app toast carries
                // the actual History / Retry buttons.
                //
                // No auto-focus, no dock bounce — per user preference.  The
                // banner is the only attention signal; the user controls when
                // to bring Said forward.
                notify_macos(
                    &app_clone,
                    "Said couldn't finish that recording",
                    &format!("{human}  Open Said to retry or view history."),
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
            &mut on_polish_event,
        )
        .await?
    };

    let n_typed = token_count.load(std::sync::atomic::Ordering::Relaxed);
    let n_failed = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let mut output_pasted = false;
    if typed_any.load(std::sync::atomic::Ordering::Relaxed) {
        if n_failed > 0 {
            // Some tokens typed, some failed — AX partially worked (user switched app?).
            // Do a full clipboard paste to ensure completeness.
            tracing::warn!(
                "[main] word-by-word partial: {n_typed} ok, {n_failed} failed — clipboard paste for safety"
            );
            if !done.polished.is_empty() {
                // Select-all and replace to avoid duplicating the partial text.
                // Uses paster::paste_replacing which sends Cmd+A first, then Cmd+V.
                // This is the key fix for the LLM-emits-duplicate bug AND for the
                // case where word-by-word typing partially fails: the safety paste
                // now actually REPLACES the partial output instead of appending.
                match paster::paste_replacing(&done.polished) {
                    Ok(_) => {
                        output_pasted = true;
                    }
                    Err(e) => {
                        tracing::warn!("[main] safety paste failed: {e}");
                    }
                }
            }
        } else {
            tracing::info!("[main] word-by-word complete — {n_typed} token(s) typed directly");
            output_pasted = true;
        }
    } else {
        // Live token typing did not produce output — paste the final result.
        tracing::info!(
            "[main] live typing produced no output — clipboard paste final result ({} chars)",
            done.polished.len()
        );
        if !done.polished.is_empty() {
            match paster::paste(&done.polished) {
                Ok(_) => {
                    output_pasted = true;
                }
                Err(e) => {
                    tracing::warn!("[main] paste fallback failed: {e}");
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
        tracing::info!(
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
        tracing::info!("[main] polished text: {:?}{}", preview, suffix);
    }

    let output_status = if output_pasted {
        "pasted"
    } else {
        "manual_paste"
    };
    let output_message = if output_pasted {
        "Pasted"
    } else {
        "Press Ctrl+Cmd+V to paste anywhere"
    };
    tracing::info!("[main] voice-output status={output_status}");
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
    let target_app = action.target_app.clone();
    let output_language = action.output_language.clone();
    let audio_id = action.audio_id.clone();
    let enriched_transcript = action.enriched_transcript.clone();
    let app_clone = app.clone();

    let typed_any = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let typed_any2 = typed_any.clone();
    let token_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let token_count2 = token_count.clone();
    let fail_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count2 = fail_count.clone();

    let mut on_polish_event = move |event| match &event {
        api::PolishEvent::Token { token } => {
            if token == "\u{1F}__RESET__\u{1F}" {
                fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
            match paster::type_text(token) {
                Ok(true) => {
                    typed_any2.store(true, std::sync::atomic::Ordering::Relaxed);
                    token_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(false) => {
                    fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!("[voice-repair] type_text error: {e}");
                }
            }
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

    finalize_typed_or_pasted_output(
        app,
        &done,
        typed_any,
        token_count,
        fail_count,
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
    let tone = Some(action.tone.clone());
    let app_clone = app.clone();

    let typed_any = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let typed_any2 = typed_any.clone();
    let token_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let token_count2 = token_count.clone();
    let fail_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let fail_count2 = fail_count.clone();

    let mut on_polish_event = move |event| match &event {
        api::PolishEvent::Token { token } => {
            let _ = app_clone.emit("voice-token", serde_json::json!({ "token": token }));
            match paster::type_text(token) {
                Ok(true) => {
                    typed_any2.store(true, std::sync::atomic::Ordering::Relaxed);
                    token_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(false) => {
                    fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    fail_count2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!("[text-refine] type_text error: {e}");
                }
            }
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

    finalize_typed_or_pasted_output(
        app,
        &done,
        typed_any,
        token_count,
        fail_count,
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

fn finalize_typed_or_pasted_output(
    app: &tauri::AppHandle,
    done: &api::PolishDone,
    typed_any: std::sync::Arc<std::sync::atomic::AtomicBool>,
    token_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    fail_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    repair_stage: LastRepairStage,
) -> Result<(), String> {
    let n_typed = token_count.load(std::sync::atomic::Ordering::Relaxed);
    let n_failed = fail_count.load(std::sync::atomic::Ordering::Relaxed);
    let mut output_pasted = false;
    if typed_any.load(std::sync::atomic::Ordering::Relaxed) {
        if n_failed > 0 {
            if !done.polished.is_empty() {
                paster::paste_replacing(&done.polished)
                    .map_err(|e| format!("paste replacing failed: {e}"))?;
                output_pasted = true;
            }
        } else {
            tracing::info!("[main] word-by-word complete — {n_typed} token(s) typed directly");
            output_pasted = true;
        }
    } else if !done.polished.is_empty() {
        paster::paste(&done.polished).map_err(|e| format!("paste failed: {e}"))?;
        output_pasted = true;
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
        "Pasted"
    } else {
        "Press Ctrl+Cmd+V to paste anywhere"
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
        let mut d = match shared2.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let snap = match result {
            Ok(done) => d.finish_ok(ProcessSummary {
                transcript: done.transcript.clone(),
                polished: done.polished,
                model: done.model_used,
                confidence: done.confidence.unwrap_or(0.0),
                transcribe_ms: done.latency_ms.transcribe as u64,
                polish_ms: done.latency_ms.polish as u64,
            }),
            Err(e) => d.finish_err(e),
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
        let mut d = match shared2.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let snap = match result {
            Ok(done) => d.finish_ok(ProcessSummary {
                transcript: done.transcript.clone(),
                polished: done.polished,
                model: done.model_used,
                confidence: done.confidence.unwrap_or(0.0),
                transcribe_ms: done.latency_ms.transcribe as u64,
                polish_ms: done.latency_ms.polish as u64,
            }),
            Err(e) => d.finish_err(e),
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
            applescript_string("Save Said audio recording"),
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
            &app2,
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
            );
        }

        let mut d = match shared2.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        let snap = match result {
            Ok(done) => d.finish_ok(ProcessSummary {
                transcript: done.transcript.clone(),
                polished: done.polished,
                model: done.model_used,
                confidence: done.confidence.unwrap_or(0.0),
                transcribe_ms: done.latency_ms.transcribe as u64,
                polish_ms: done.latency_ms.polish as u64,
            }),
            Err(e) => d.finish_err(e),
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

    // OS-level fallback for when the Said window isn't focused.
    notify_macos(
        &app,
        "Added to vocabulary",
        &format!("Said will recognise \"{term}\" on your next recording."),
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
            &format!("Said will keep \"{term}\" even if you stop using it."),
        );
    }
    Ok(starred)
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
    std::path::PathBuf::from(format!(
        "{}/Library/Logs/Said",
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    ))
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
    let (backend, backend_truncated) = read_recent_log(&backend_path, "polish-backend build=");

    let combined = format!(
        "── Said desktop ({}) ──\n{}\n\n── polish-backend ({}) ──\n{}",
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
            .filter(|(_, process)| process.name().to_string_lossy() == "said-backend")
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

const EDIT_WATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(5);
const EDIT_WATCH_MAX_DURATION: Duration = Duration::from_secs(30);
// AX reads can be expensive in Electron/Chromium apps, so keep this responsive
// without polling at frame-rate cadence.
const EDIT_WATCH_FAST_INTERVAL: Duration = Duration::from_millis(150);
const EDIT_WATCH_SLOW_INTERVAL: Duration = Duration::from_millis(500);
const EDIT_WATCH_BLOCKING_TIMEOUT: Duration = Duration::from_millis(500);

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
        Err(_) => {
            tracing::warn!("[edit-watch] watcher state lock poisoned; cannot cancel");
            return;
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
) {
    let token = {
        let st = app.state::<EditWatcherState>();
        let mut guard = match st.0.lock() {
            Ok(g) => g,
            Err(_) => {
                tracing::warn!("[edit-watch] watcher state lock poisoned; skipping watcher");
                return;
            }
        };
        if let Some(prev) = guard.take() {
            tracing::info!("[edit-watch] cancelling previous watcher");
            prev.cancel();
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
) {
    use std::time::Instant;

    // Let the paste animation settle and focus move into the text field.
    if !cancellable_sleep(&token, Duration::from_millis(700)).await {
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
    // best_candidate = last field value that still shared words with polished text.
    // Needed because apps like Slack clear the input after Send, making last_val
    // a UI placeholder ("Type / for commands") that replaces the actual edit.
    let mut best_candidate = post_paste.clone();
    let mut idle_at = Instant::now();
    let started = Instant::now();
    let mut last_change_at = Instant::now();
    let mut current_interval = EDIT_WATCH_FAST_INTERVAL;
    let mut last_pid = initial_pid;
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

        // Read the current field value from the locked target app when we have
        // one. This avoids accidentally sampling our HUD or a newly focused app.
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
        if now_val != last_val {
            idle_at = Instant::now();
            last_change_at = Instant::now();
            current_interval = EDIT_WATCH_FAST_INTERVAL;
            // Only promote to best_candidate if the value still shares words
            // with the polished text (guards against Send-cleared placeholders).
            if shares_word_overlap(&now_val, &polished) {
                best_candidate = now_val.clone();
            }
            last_val = now_val;
        } else if last_change_at.elapsed() > Duration::from_secs(2) {
            current_interval = EDIT_WATCH_SLOW_INTERVAL;
        }

        let done = idle_at.elapsed() > EDIT_WATCH_IDLE_TIMEOUT
            || started.elapsed() > EDIT_WATCH_MAX_DURATION;

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
        user_kept = extract_kept(&polished, &post_paste, &effective_val);
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

                if resp.notify {
                    // In-app toast per learnable term (matches website UI,
                    // includes Undo affordance).  We surface the first term
                    // — most edits produce one; on rare multi-promote edits
                    // the others land silently in vocab.
                    let first_term = resp.promoted_terms.first().cloned();
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
                    // OS-level fallback for when the Said window isn't focused.
                    // Plain human copy — never the LLM's raw reason string,
                    // never internal class names, never punctuation flair.
                    let (title, body) = match resp.class.as_str() {
                        "STT_ERROR" => (
                            "Said learned a new word",
                            match &first_term {
                                Some(t) => {
                                    format!("Said will recognise \"{t}\" on your next recording.")
                                }
                                None => "Said remembered your correction.".to_string(),
                            },
                        ),
                        "POLISH_ERROR" => (
                            "Said updated a writing preference",
                            "Said will use your wording next time.".to_string(),
                        ),
                        _ => (
                            "Said learned from your edit",
                            "Said remembered your correction.".to_string(),
                        ),
                    };
                    notify_macos(&app, title, &body);
                }

                // Surface queued (k-event sighting recorded but not yet promoted)
                // so the user knows the system noticed their correction. Without
                // this the silent k-event behavior looks broken — the classifier
                // ran, found a real STT error, and "did nothing" from the user's
                // POV. Soft toast only; no OS banner.
                if let Some(term) = resp.queued_terms.first() {
                    if !term.trim().is_empty() {
                        let _ = app.emit(
                            "vocab-toast",
                            serde_json::json!({
                                "kind":   "queued",
                                "term":   term,
                                "source": "auto",
                            }),
                        );
                    }
                }

                if resp.learned || resp.pending_id.is_some() {
                    let _ = app.emit("pending-edits-changed", ());
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
    let ref_words: std::collections::HashSet<String> = reference
        .split_whitespace()
        .filter(|w| w.chars().count() > 3)
        .map(|w| w.to_lowercase())
        .collect();
    if ref_words.is_empty() {
        return !candidate.is_empty();
    }
    candidate
        .split_whitespace()
        .any(|w| ref_words.contains(&w.to_lowercase()))
}

/// Given what we pasted (`polished`), where the field was right after paste
/// (`post_paste`), and the final field value (`last_val`), extract the user's
/// edited version of our text.
fn extract_kept(polished: &str, post_paste: &str, last_val: &str) -> String {
    // Find where our polished text starts in the field.
    let Some(offset) = post_paste.find(polished.trim()) else {
        // Can't locate it precisely — return the full field value.
        return last_val.to_string();
    };

    let prefix = &post_paste[..offset];
    let after_end = offset + polished.trim().len();
    let suffix = &post_paste[after_end..];

    // In last_val, strip the same prefix and suffix to get the edited middle.
    if let Some(lv_after_prefix) = last_val.strip_prefix(prefix) {
        if let Some(edited) = lv_after_prefix.strip_suffix(suffix) {
            return edited.trim().to_string();
        }
        // Suffix changed too — return everything after the prefix.
        return lv_after_prefix.trim().to_string();
    }

    // Prefix changed — return full field value as a fallback.
    last_val.to_string()
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

fn main() {
    install_rustls_crypto_provider();

    // 1. Load env vars from .env files
    said_core::load_env();

    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        backend_guard::kill_from_pid_file();
        default_panic_hook(info);
    }));

    // 2. Tracing — write to ~/Library/Logs/Said/said.log so logs survive in bundled app
    let log_dir = format!(
        "{}/Library/Logs/Said",
        std::env::var("HOME").unwrap_or_else(|_| ".".into())
    );
    std::fs::create_dir_all(&log_dir).ok();
    let log_path = format!("{log_dir}/said.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("cannot open said.log");
    // Two tracing layers: log file (always) + stderr (for `cargo run` visibility).
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
            .init();
    }
    tracing::info!("[main] said desktop starting — log file: {log_path}");

    // 3. Shared state
    let shared_app = Arc::new(Mutex::new(DesktopApp::new()));
    let backend_arc = Arc::new(Mutex::new(None::<BackendEndpoint>));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup({
            let shared   = Arc::clone(&shared_app);
            let back_arc = Arc::clone(&backend_arc);
            move |app| {
                #[cfg(target_os = "macos")]
                {
                    // Accessory policy keeps the recorder HUD available across
                    // macOS Spaces/fullscreen desktops. A regular foreground
                    // app made the HUD Space-bound on this Tauri/WebKit stack.
                    app.set_activation_policy(tauri::ActivationPolicy::Accessory);
                    tracing::info!("[main] macOS activation policy set to Accessory");
                }

                // ── Spawn backend daemon ──────────────────────────────────────
                // ── Permission status at launch (visible in ~/Library/Logs/Said/said.log) ──
                let ax_ok = paster::is_accessibility_granted();
                let im_ok = hotkey::is_input_monitoring_granted();
                tracing::info!("[perm] Accessibility={ax_ok} InputMonitoring={im_ok}");
                if !ax_ok {
                    tracing::warn!("[perm] Accessibility NOT granted — paste will fail. Grant in System Settings → Privacy → Accessibility");
                }
                if !im_ok {
                    tracing::warn!("[perm] Input Monitoring NOT granted — hotkeys (Caps Lock, Option+1-5, Ctrl+Cmd+V) will not work. Grant in System Settings → Privacy → Input Monitoring");
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
                                #[cfg(target_os = "macos")]
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
                        &MenuItem::with_id(app, "show", "Open Said", true, None::<&str>)?,
                        &PredefinedMenuItem::separator(app)?,
                        &MenuItem::with_id(app, "quit", "Quit Said", true, None::<&str>)?,
                    ])?,
                };

                // Brand mark — embedded retina PNG, marked as template so
                // macOS auto-tints to match menu bar appearance.
                let tray_icon = tauri::image::Image::from_bytes(
                    include_bytes!("../icons/tray@2x.png")
                ).ok();

                let mut tray_builder = TrayIconBuilder::with_id("said")
                    .tooltip("Said — Voice Polish Studio")
                    .menu(&initial_menu)
                    .show_menu_on_left_click(true);   // ← left-click opens menu

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

                // ── Hold-to-record hotkey (macOS only) ────────────────────────
                #[cfg(target_os = "macos")]
                {
                    let shared_hold_press = Arc::clone(&app.state::<SharedApp>().0);
                    let shared_hold_release = Arc::clone(&app.state::<SharedApp>().0);
                    let back_hold = Arc::clone(&app.state::<BackendState>().0);
                    let app_press = app.handle().clone();
                    let app_release = app.handle().clone();
                    hotkey::start_hold_listener(
                        Arc::new(move || {
                            let shared = Arc::clone(&shared_hold_press);
                            let app_h = app_press.clone();
                            std::thread::spawn(move || {
                                do_start_recording(&shared, &app_h);
                            });
                        }),
                        Arc::new(move || {
                            let shared = Arc::clone(&shared_hold_release);
                            let back = Arc::clone(&back_hold);
                            let app_h = app_release.clone();
                            std::thread::spawn(move || {
                                do_finish_recording(shared, app_h, back);
                            });
                        }),
                    );

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
        .manage(StreamingState(Mutex::new(None)))
        .manage(DeepgramSessionState(dg_stream::DeepgramSession::spawn()))
        .manage(PerformanceState(Mutex::new(sysinfo::System::new_all())))
        .manage(TrayCache(Mutex::new(TrayCacheInner::default())))
        .manage(LatestResult(std::sync::Arc::new(Mutex::new(None))))
        .manage(LastActionState(Mutex::new(None)))
        .manage(HotPathCache(Arc::new(tokio::sync::RwLock::new(HotPathCacheInner::default()))))
        .manage(StatusBarHideGen(Arc::new(AtomicU64::new(0))))
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            get_snapshot,
            dismiss_status_bar,
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
            // Vocabulary management
            list_vocabulary,
            add_vocabulary_term,
            delete_vocabulary_term,
            star_vocabulary_term,
            // Invite a friend
            send_invite_email,
            // External URL opener (mailto:, https://) — Tauri webview blocks window.open
            open_external,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Voice Polish desktop")
        .run(|app, event| match event {
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
    fn reset_disables_future_live_typing() {
        let mut guard = LiveTypingGuard::default();
        assert_eq!(guard.on_token("Hello"), LiveTypingDecision::TypeToken);
        assert_eq!(
            guard.on_token(STREAM_RESET_SENTINEL),
            LiveTypingDecision::ResetAndDisable
        );
        assert_eq!(guard.on_token("world"), LiveTypingDecision::PreviewOnly);
    }
}
