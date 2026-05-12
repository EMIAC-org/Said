//! Windows hotkey listener — stubbed scaffolding.
//!
//! P0 only — this module declares the same public API surface as
//! `crates/hotkey/src/macos.rs` so the workspace compiles on `windows-latest`.
//! Real implementations land in P2:
//!
//!   * `WH_KEYBOARD_LL` low-level hook on a dedicated message-pump thread
//!   * `WH_MOUSE_LL` low-level hook for `KeyEvt::MouseClick`
//!   * Dispatch thread that drains a `crossbeam_queue::ArrayQueue` and
//!     populates the cross-platform `KEY_BUF` (see `crate::shared`)
//!
//! Stubs are deliberately *not* `unimplemented!()` — the branching-strategy
//! reference doc forbids shipping panicking stubs. Each stub is a sensible
//! no-op so consumers (Tauri shell, `said` CLI) can run on Windows without
//! crashing, even though the hotkey feature is inert until P2.

use std::sync::Arc;

use crate::RecordHotkey;

/// Windows has no equivalent of macOS Input Monitoring TCC; nothing to gate.
/// Real LL-hook installation does not require explicit user permission, only
/// (in practice) a code-signed binary so SmartScreen doesn't flag it.
pub fn is_input_monitoring_granted() -> bool {
    true
}

/// No-op on Windows — `RecordHotkey` is stored locally by the Tauri layer
/// and the future Windows hotkey listener will read it back via this setter.
pub fn set_record_hotkey(hotkey: RecordHotkey) {
    tracing::debug!("[hotkey] set_record_hotkey({hotkey:?}) — Windows impl pending (P2)");
}

/// Register the Option+1..5 tray callback. Stored but never fired in P0.
pub fn register_shortcut_callback(_cb: Arc<dyn Fn(u8) + Send + Sync>) {
    tracing::debug!("[hotkey] register_shortcut_callback — Windows impl pending (P2)");
}

/// Register the paste-latest hotkey callback. Stored but never fired in P0.
pub fn register_paste_callback(_cb: Arc<dyn Fn() + Send + Sync>) {
    tracing::debug!("[hotkey] register_paste_callback — Windows impl pending (P2)");
}

/// Toggle-on-every-press listener. No-op until P2.
pub fn start_listener(_callback: Arc<dyn Fn() + Send + Sync>) {
    tracing::warn!("[hotkey] start_listener — Windows impl pending (P2); hotkey is inert");
}

/// Hold-to-record listener. No-op until P2.
pub fn start_hold_listener(
    _on_press: Arc<dyn Fn() + Send + Sync>,
    _on_release: Arc<dyn Fn() + Send + Sync>,
) {
    tracing::warn!("[hotkey] start_hold_listener — Windows impl pending (P2); hotkey is inert");
}
