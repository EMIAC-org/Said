//! `said-hotkey` — global hotkey listener for Said.
//!
//! Two listening modes (set independently of the chosen [`RecordHotkey`]):
//!   - [`start_listener`]      — fires a callback on every press (toggle mode)
//!   - [`start_hold_listener`] — fires `on_press` while held, `on_release` when lifted
//!
//! ## Platform layout
//!
//! Each OS implements the same public surface in its own module:
//!
//! | Module        | OS gate                  | Status                             |
//! |---------------|--------------------------|------------------------------------|
//! | [`macos`]     | `target_os = "macos"`    | Full CGEventTap implementation     |
//! | [`windows`]   | `target_os = "windows"`  | Stub scaffolding (real impl: P2)   |
//! | (none)        | other unix-like targets  | Unsupported (build error expected) |
//!
//! Shared cross-platform types ([`KeyEvt`], [`TimedKeyEvt`], [`key_buffer`]) live
//! in the private [`shared`] module and are re-exported here so callers never
//! need to know which OS is active.

mod shared;
pub use shared::{KeyEvt, TimedKeyEvt, key_buffer};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{
    is_input_monitoring_granted, register_paste_callback, register_shortcut_callback,
    set_record_hotkey, start_hold_listener, start_listener,
};

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::{
    is_input_monitoring_granted, register_paste_callback, register_shortcut_callback,
    set_record_hotkey, start_hold_listener, start_listener,
};

/// Which physical key triggers recording. Cross-platform enum; some variants are
/// platform-specific.
///
/// | Variant       | macOS                       | Windows                              |
/// |---------------|-----------------------------|--------------------------------------|
/// | `CapsLock`    | Caps Lock (held) — default  | Caps Lock — fragile under RDP/Citrix |
/// | `RightOption` | Right Option (held)         | Right Alt — conflicts with AltGr     |
/// | `Function`    | Fn / Globe (held)           | Not available — ignored              |
/// | `RightCtrl`   | (unused; falls back to CL)  | Right Ctrl (held) — **default**      |
/// | `F13`         | F13 (held)                  | F13 (held) — clean but rare keyboard |
/// | `Pause`       | (unused)                    | Pause/Break (held) — universal       |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordHotkey {
    CapsLock,
    RightOption,
    Function,
    /// Windows-specific: default on that platform. macOS treats this as
    /// equivalent to `CapsLock` (CGEventTap can't bind RightCtrl reliably).
    RightCtrl,
    /// Available on both platforms when the keyboard has an F13 key.
    F13,
    /// Windows-only: Pause/Break key. Ignored on macOS (Apple keyboards
    /// don't ship a Pause key in the standard layout).
    Pause,
}
