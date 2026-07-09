#[cfg(target_os = "macos")]
use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::sync::atomic::Ordering;

#[cfg(target_os = "macos")]
use tauri::{AppHandle, Manager, WebviewWindow};

#[cfg(target_os = "macos")]
use crate::{
    SharedApp, StatusBarHideGen, create_status_bar, desktop, diag, run_on_main_guarded,
    schedule_present_status_bar_macos, status_bar_persistent_hold, status_bar_pinned,
};

#[cfg(target_os = "macos")]
pub(crate) struct MacHudManager {
    app: AppHandle,
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
        schedule_present_status_bar_macos(&self.app, &win, state, state != "placement");
        diag::breadcrumb(format!("status_bar:sync_main:{state}:end"));
    }

    pub(crate) fn present(&self, reason: &str, resync: bool) -> Result<(), String> {
        let win = self.ensure_window_present()?;
        tracing::debug!("[status-bar] native present reason={reason} resync={resync}");
        schedule_present_status_bar_macos(&self.app, &win, reason, resync);
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
        if let Some(win) = self.app.get_webview_window("status-bar") {
            win.hide()
                .map_err(|e| format!("hide status bar failed: {e}"))?;
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
                create_status_bar(&self.app);
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
                tracing::warn!(
                    "[status-bar] sync requested for state={state}, but window was not found"
                );
                None
            }
        }
    }

    fn ensure_window_present(&self) -> Result<WebviewWindow, String> {
        if self.app.get_webview_window("status-bar").is_none() {
            create_status_bar(&self.app);
        }
        self.app
            .get_webview_window("status-bar")
            .ok_or_else(|| "status-bar window not found".to_string())
    }

    fn sync_idle(&self, win: &WebviewWindow) {
        if status_bar_pinned() || status_bar_persistent_hold(&self.app) {
            diag::breadcrumb("status_bar:sync_main:idle:held_visible");
            tracing::debug!("[status-bar] idle state — pinned/held, keeping visible");
            schedule_present_status_bar_macos(&self.app, win, "idle", false);
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
                        if let Some(win) = app_main.get_webview_window("status-bar") {
                            diag::breadcrumb("status_bar:idle_hide:begin");
                            match win.hide() {
                                Ok(_) => {
                                    diag::breadcrumb("status_bar:idle_hide:end");
                                    tracing::debug!("[status-bar] hidden after idle")
                                }
                                Err(e) => {
                                    diag::breadcrumb("status_bar:idle_hide:failed");
                                    tracing::warn!("[status-bar] hide after idle failed: {e}")
                                }
                            }
                        }
                    })
                {
                    tracing::warn!("[status-bar] schedule idle hide failed: {e}");
                }
            }
        });
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
}
