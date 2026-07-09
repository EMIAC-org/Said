//! Fault injection for torture-testing the resilience fixes on the *real* binary.
//!
//! Completely INERT unless `AIRNOTE_CHAOS=1` is set at launch — production installs
//! never set it, and every injector refuses without it. The faults are
//! non-destructive (they only exercise the recovery paths) and carry no user data.
//!
//! Each injector reproduces one real failure mode so its recovery can be verified
//! deterministically instead of waiting for a real crash:
//!
//! | kind              | reproduces                  | expected recovery signal              |
//! |-------------------|-----------------------------|---------------------------------------|
//! | `main_panic`      | panic in an AppKit callback | `panic.recovered` + `guard:recovered:*` |
//! | `pipeline_panic`  | finish task dies mid-polish | tokio-caught, app survives, `state.healed` |
//! | `stick_processing`| wedged state machine        | `state.healed` (watchdog reset)       |
//! | `plant_orphan`    | crash mid-dictation         | `recovery:*` on next launch           |
//! | `drop_hud`        | HUD hidden while active     | `hud_watchdog:recover`                |
//! | `emit_diag`       | —                           | `chaos.test_event` on the dashboard   |
//!
//! Soak mode (`AIRNOTE_CHAOS_SOAK=1`) self-injects these on a loop so a monitor
//! script can torture a running app and assert it self-heals (see
//! `tools/e2e-stress/soak.sh`).

use tauri::{Emitter, Manager};

/// Chaos is opt-in via env so it can never fire in a normal install.
pub fn enabled() -> bool {
    std::env::var("AIRNOTE_CHAOS")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// Force the state machine into `Processing` without ever finishing — the exact
/// wedge a dead finish-pipeline leaves behind.
fn stick_processing(app: &tauri::AppHandle) {
    let snap = app.try_state::<crate::SharedApp>().map(|shared| {
        let mut d = match shared.0.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        d.state = crate::desktop::AppState::Processing;
        d.snapshot()
    });
    if let Some(snap) = snap {
        let _ = app.emit("app-state", &snap);
    }
}

/// Run one named fault. Returns a short human-readable status. No-op refusal when
/// chaos is not enabled.
pub fn inject(app: &tauri::AppHandle, kind: &str) -> String {
    if !enabled() {
        return "chaos disabled — relaunch with AIRNOTE_CHAOS=1".to_string();
    }
    tracing::warn!("[chaos] injecting fault '{kind}'");
    crate::diag::breadcrumb(format!("chaos:inject:{kind}"));
    match kind {
        // Job 1A — seatbelt. Panic inside a guarded AppKit callback; the app must
        // survive and report `panic.recovered`.
        "main_panic" => {
            let _ = crate::run_on_main_guarded(app, "chaos.main_panic", move || {
                panic!("chaos: induced main-thread panic");
            });
            "scheduled main-thread panic — seatbelt should catch it".into()
        }
        // Job 1C — a finish task that dies mid-processing. tokio catches the task
        // panic (process survives) and the state watchdog heals the wedge.
        "pipeline_panic" => {
            stick_processing(app);
            tauri::async_runtime::spawn(async move {
                panic!("chaos: induced pipeline task panic");
            });
            "wedged processing + panicked task — watchdog should heal".into()
        }
        // Job 1C — bare wedged state, no panic.
        "stick_processing" => {
            stick_processing(app);
            "forced processing state — watchdog should reset to idle".into()
        }
        // Job 2 — plant a crash-orphan recording; recovered on next launch.
        "plant_orphan" => {
            crate::recovery::plant_synthetic_orphan(3);
            "planted a 3s synthetic orphan — relaunch to verify recovery".into()
        }
        // HUD visibility watchdog — hide the pill while the app is active; the
        // watchdog should notice the hidden HUD and bring it back
        // (`hud_watchdog:recover`).
        "drop_hud" => {
            stick_processing(app);
            let app2 = app.clone();
            let _ = crate::run_on_main_guarded(app, "chaos.drop_hud", move || {
                if let Some(win) = app2.get_webview_window("status-bar") {
                    let _ = win.hide();
                }
            });
            "hid HUD while active — HUD watchdog should restore it".into()
        }
        // Diagnostics pipeline end-to-end.
        "emit_diag" => {
            said_core::reporter::report_event(
                "chaos.test_event",
                said_core::reporter::Severity::Info,
                serde_json::json!({ "note": "chaos diagnostics path check" }),
            );
            "emitted chaos.test_event — check the diagnostics dashboard".into()
        }
        other => format!("unknown chaos kind '{other}'"),
    }
}

/// In soak mode, self-inject faults on a loop so a monitor can verify the app
/// tortures-and-heals indefinitely without a human clicking anything.
pub fn maybe_start_soak(app: &tauri::AppHandle) {
    let soak = std::env::var("AIRNOTE_CHAOS_SOAK")
        .map(|v| v == "1")
        .unwrap_or(false);
    if !soak || !enabled() {
        return;
    }
    let interval_secs: u64 = std::env::var("AIRNOTE_CHAOS_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&n| n >= 5)
        .unwrap_or(20);
    tracing::warn!(
        "[chaos] ⚠ SOAK MODE ENABLED — self-injecting faults every {interval_secs}s. NEVER ship with AIRNOTE_CHAOS set."
    );
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // emit_diag first (harmless) so the dashboard path is exercised before any
        // disruptive fault; then cycle through the recovery paths.
        let kinds = [
            "emit_diag",
            "main_panic",
            "stick_processing",
            "drop_hud",
            "pipeline_panic",
        ];
        let mut tick = 0usize;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
            let kind = kinds[tick % kinds.len()];
            tick += 1;
            let status = inject(&app, kind);
            tracing::warn!("[chaos] soak tick {tick}: {kind} → {status}");
        }
    });
}
