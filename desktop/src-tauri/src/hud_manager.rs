#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;

#[cfg(target_os = "macos")]
use tauri::{AppHandle, Manager, WebviewWindow};
#[cfg(target_os = "macos")]
use tauri_nspanel::{
    CollectionBehavior, ManagerExt as PanelManagerExt, PanelBuilder, PanelLevel, StyleMask,
};

#[cfg(target_os = "macos")]
use crate::{
    NOTCH_FIRST_CLASS, STATUS_BAR_HEIGHT, STATUS_BAR_WIDTH, SharedApp, StatusBarAnchor,
    StatusBarHideGen, StatusBarInteractive, StatusBarPanel, apply_status_bar_position,
    clear_status_bar_position, desktop, diag, emit_status_bar_resync, run_on_main_guarded,
    save_status_bar_anchor, status_bar_persistent_hold, status_bar_pinned,
    status_bar_target_origin,
};

#[cfg(target_os = "macos")]
pub(crate) struct MacHudManager {
    app: AppHandle,
}

#[cfg(target_os = "macos")]
pub(crate) fn configure_window(win: &WebviewWindow) {
    diag::breadcrumb("status_bar:configure:begin");
    // Legacy compatibility hook. Runtime AppKit style/level/collection retuning
    // is intentionally avoided; PanelBuilder owns that configuration at creation.
    MacHudManager::apply_interactive_to_window(win, status_bar_interactive(win.app_handle()));
    diag::breadcrumb("status_bar:configure:end");
}

#[cfg(target_os = "macos")]
pub(crate) fn show_panel(app: &AppHandle) -> bool {
    match app.get_webview_panel("status-bar") {
        Ok(panel) => {
            diag::breadcrumb("status_bar:panel_show:begin");
            panel.show();
            panel.order_front_regardless();
            diag::breadcrumb("status_bar:panel_show:end");
            true
        }
        Err(_) => {
            diag::breadcrumb("status_bar:panel_show:missing");
            tracing::warn!("[status-bar] panel handle missing; falling back to webview window");
            false
        }
    }
}

#[cfg(target_os = "macos")]
impl MacHudManager {
    pub(crate) fn new(app: &AppHandle) -> Self {
        Self { app: app.clone() }
    }

    pub(crate) fn sync_on_main(&self, state: &str) {
        diag::breadcrumb(format!("status_bar:sync_main:{state}:begin"));
        let Some(win) = self.window_for_sync(state) else {
            return;
        };

        tracing::debug!("[status-bar] sync state={state}");
        if state == "idle" {
            self.sync_idle(&win);
            return;
        }

        self.invalidate_pending_idle_hide();
        self.present_on_main(&win, state, state != "placement");
        diag::breadcrumb(format!("status_bar:sync_main:{state}:end"));
    }

    pub(crate) fn ensure_created(&self) {
        if NOTCH_FIRST_CLASS.load(Ordering::Relaxed) {
            // Notch sidecar is the HUD; never bring up the pill.
            return;
        }
        if self.app.get_webview_window("status-bar").is_some() {
            tracing::info!("[status-bar] create skipped; window already exists");
            return;
        }

        let idle_w = STATUS_BAR_WIDTH;
        let idle_h = STATUS_BAR_HEIGHT;
        let (x, y) = status_bar_target_origin(&self.app, idle_w, idle_h);

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

        match PanelBuilder::<_, StatusBarPanel>::new(&self.app, "status-bar")
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
                    .background_throttling(
                        tauri::utils::config::BackgroundThrottlingPolicy::Disabled,
                    )
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
                if status_bar_pinned() {
                    tracing::info!("[status-bar] dev pin active — showing at idle");
                } else {
                    panel.hide();
                }
                if let Some(win) = self.app.get_webview_window("status-bar") {
                    match win.url() {
                        Ok(url) => tracing::info!("[status-bar] resolved url={url}"),
                        Err(e) => tracing::warn!("[status-bar] could not read window url: {e}"),
                    }
                    Self::apply_interactive_to_window(&win, status_bar_interactive(&self.app));
                }
            }
            Err(e) => tracing::warn!("[status-bar] could not create NSPanel: {e}"),
        }
    }

    pub(crate) fn present(&self, reason: &str, resync: bool) -> Result<(), String> {
        let win = self.ensure_window_present()?;
        tracing::debug!("[status-bar] native present reason={reason} resync={resync}");
        let app_for_main = self.app.clone();
        let app_in_closure = app_for_main.clone();
        let reason = reason.to_string();
        if let Err(e) = run_on_main_guarded(&app_for_main, "status_bar.present", move || {
            MacHudManager::new(&app_in_closure).present_on_main(&win, &reason, resync);
        }) {
            return Err(format!("schedule present failed: {e}"));
        }
        Ok(())
    }

    pub(crate) fn dismiss_on_main(&self) -> Result<(), String> {
        if status_bar_pinned() || status_bar_persistent_hold(&self.app) {
            return Ok(());
        }
        if self.is_active() {
            tracing::debug!("[status-bar] dismiss skipped — app state is active");
            return Ok(());
        }
        self.hide_window_on_main("dismiss")?;
        Ok(())
    }

    pub(crate) fn set_interactive(&self, interactive: bool) {
        if let Some(state) = self.app.try_state::<StatusBarInteractive>() {
            state.0.store(interactive, Ordering::SeqCst);
        }
        if let Some(win) = self.app.get_webview_window("status-bar") {
            let app_for_main = self.app.clone();
            let win_for_main = win.clone();
            if let Err(e) =
                run_on_main_guarded(&app_for_main, "status_bar.interactive", move || {
                    Self::apply_interactive_to_window(&win_for_main, interactive);
                })
            {
                tracing::warn!("[status-bar] schedule interactive state failed: {e}");
            }
        }
    }

    pub(crate) fn resize_on_main(&self, width: f64, height: f64) -> Result<(), String> {
        let win = self
            .app
            .get_webview_window("status-bar")
            .ok_or_else(|| "status-bar window not found".to_string())?;
        let (center_x, bottom_y) = Self::read_window_bottom_anchor(&win).unwrap_or_else(|| {
            let (x, y) = status_bar_target_origin(&self.app, width, height);
            (x + width / 2.0, y + height)
        });
        win.set_size(tauri::Size::Logical(tauri::LogicalSize::new(width, height)))
            .map_err(|e| format!("resize status bar failed: {e}"))?;
        let (x, y) = Self::origin_from_bottom_anchor(center_x, bottom_y, width, height);
        win.set_position(tauri::Position::Logical(tauri::LogicalPosition::new(x, y)))
            .map_err(|e| format!("post-resize position failed: {e}"))?;
        Ok(())
    }

    pub(crate) fn set_position_on_main(&self, x: f64, y: f64) -> Result<(), String> {
        let win = self
            .app
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

    pub(crate) fn reset_position_on_main(&self) -> Result<(), String> {
        clear_status_bar_position()?;
        if let Some(win) = self.app.get_webview_window("status-bar") {
            apply_status_bar_position(&self.app, &win)?;
        }
        Ok(())
    }

    fn window_for_sync(&self, state: &str) -> Option<WebviewWindow> {
        match self.app.get_webview_window("status-bar") {
            Some(win) => Some(win),
            None if state != "idle" => {
                diag::breadcrumb(format!("status_bar:sync_main:{state}:missing_recreate"));
                tracing::warn!(
                    "[status-bar] sync requested for active state={state}, but window was not found — recreating"
                );
                self.ensure_created();
                let Some(win) = self.app.get_webview_window("status-bar") else {
                    diag::breadcrumb(format!("status_bar:sync_main:{state}:recreate_failed"));
                    tracing::warn!(
                        "[status-bar] recreate failed; still no status-bar window for state={state}"
                    );
                    return None;
                };
                Some(win)
            }
            None => {
                diag::breadcrumb(format!("status_bar:sync_main:{state}:missing_idle"));
                tracing::debug!(
                    "[status-bar] sync requested for state={state}, but window is not created"
                );
                None
            }
        }
    }

    fn ensure_window_present(&self) -> Result<WebviewWindow, String> {
        if self.app.get_webview_window("status-bar").is_none() {
            self.ensure_created();
        }
        self.app
            .get_webview_window("status-bar")
            .ok_or_else(|| "status-bar window not found".to_string())
    }

    fn sync_idle(&self, win: &WebviewWindow) {
        if status_bar_pinned() || status_bar_persistent_hold(&self.app) {
            diag::breadcrumb("status_bar:sync_main:idle:held_visible");
            tracing::debug!("[status-bar] idle state — pinned/held, keeping visible");
            self.present_on_main(win, "idle", false);
            return;
        }

        tracing::debug!("[status-bar] idle state — scheduling native hide");
        diag::breadcrumb("status_bar:sync_main:idle:schedule_hide");
        let my_gen = self
            .app
            .try_state::<StatusBarHideGen>()
            .map(|s| s.0.fetch_add(1, Ordering::Relaxed) + 1)
            .unwrap_or(0);
        let hide_gen_arc = self
            .app
            .try_state::<StatusBarHideGen>()
            .map(|s| Arc::clone(&s.0));
        let app = self.app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(2600)).await;
            if let Some(counter) = &hide_gen_arc
                && counter.load(Ordering::Relaxed) != my_gen
            {
                return;
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
                let app_main = app.clone();
                if let Err(e) =
                    run_on_main_guarded(&app_main.clone(), "status_bar.idle_hide", move || {
                        let manager = MacHudManager::new(&app_main);
                        if let Err(e) = manager.hide_window_on_main("idle") {
                            tracing::warn!("[status-bar] hide after idle failed: {e}");
                        }
                    })
                {
                    tracing::warn!("[status-bar] schedule idle hide failed: {e}");
                }
            }
        });
    }

    fn present_on_main(&self, win: &WebviewWindow, state: &str, resync: bool) {
        diag::breadcrumb(format!("status_bar:present:{state}:begin"));
        match apply_status_bar_position(&self.app, win) {
            Ok(_) => tracing::debug!("[status-bar] repositioned"),
            Err(e) => tracing::warn!("[status-bar] reposition failed: {e}"),
        }
        diag::breadcrumb(format!("status_bar:present:{state}:repositioned"));

        Self::apply_interactive_to_window(win, status_bar_interactive(&self.app));
        if let Err(e) = win.set_always_on_top(true) {
            tracing::warn!("[status-bar] set_always_on_top failed: {e}");
        }

        diag::breadcrumb(format!("status_bar:present:{state}:show"));
        if !show_panel(&self.app) {
            match win.show() {
                Ok(_) => tracing::debug!("[status-bar] show ok for state={state}"),
                Err(e) => tracing::warn!("[status-bar] show failed for state={state}: {e}"),
            }
        }

        if resync {
            diag::breadcrumb(format!("status_bar:present:{state}:resync"));
            emit_status_bar_resync(&self.app, state);
        }
        diag::breadcrumb(format!("status_bar:present:{state}:end"));
    }

    fn invalidate_pending_idle_hide(&self) {
        if let Some(counter) = self
            .app
            .try_state::<StatusBarHideGen>()
            .map(|s| Arc::clone(&s.0))
        {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn is_active(&self) -> bool {
        self.app
            .try_state::<SharedApp>()
            .and_then(|shared| {
                shared
                    .0
                    .lock()
                    .ok()
                    .map(|d| d.state != desktop::AppState::Idle)
            })
            .unwrap_or(false)
    }

    fn apply_interactive_to_window(win: &WebviewWindow, interactive: bool) {
        let _ = win.set_ignore_cursor_events(!interactive);
        use objc::Message;
        use objc::runtime::{Object, Sel};
        if let Ok(ns_window) = win.ns_window()
            && !ns_window.is_null()
        {
            unsafe {
                let ns_window = &*(ns_window as *mut Object);
                let _: Result<(), _> = ns_window
                    .send_message(Sel::register("setIgnoresMouseEvents:"), (!interactive,));
            }
        }
        tracing::debug!("[status-bar] interactive={interactive}");
    }

    fn read_window_bottom_anchor(win: &WebviewWindow) -> Option<(f64, f64)> {
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

    fn origin_from_bottom_anchor(
        center_x: f64,
        bottom_y: f64,
        width: f64,
        height: f64,
    ) -> (f64, f64) {
        (center_x - width / 2.0, bottom_y - height)
    }

    fn hide_window_on_main(&self, reason: &str) -> Result<(), String> {
        let Some(win) = self.app.get_webview_window("status-bar") else {
            return Ok(());
        };

        diag::breadcrumb(format!("status_bar:{reason}_hide:begin"));
        match self.app.get_webview_panel("status-bar") {
            Ok(panel) => panel.hide(),
            Err(_) => win
                .hide()
                .map_err(|e| format!("hide status bar failed: {e}"))?,
        }
        diag::breadcrumb(format!("status_bar:{reason}_hide:end"));
        tracing::debug!("[status-bar] hidden after {reason}");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn status_bar_collection_behavior() -> CollectionBehavior {
    // `.stationary` + `.can_join_all_spaces` conflict in release builds:
    // macOS silently ignores setCollectionBehavior after Space/fullscreen transitions
    // (Tauri #5566). Use only what is needed: pin to all spaces, allow over fullscreen.
    CollectionBehavior::new()
        .can_join_all_spaces()
        .full_screen_auxiliary()
}

#[cfg(target_os = "macos")]
fn status_bar_interactive(app: &AppHandle) -> bool {
    app.try_state::<StatusBarInteractive>()
        .map(|s| s.0.load(Ordering::SeqCst))
        .unwrap_or(false)
}
