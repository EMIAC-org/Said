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
/// platform-specific:
///
/// | Variant       | macOS                       | Windows                            |
/// |---------------|-----------------------------|------------------------------------|
/// | `CapsLock`    | Caps Lock (held)            | (planned P2) Caps Lock — advanced  |
/// | `RightOption` | Right Option (held)         | (planned P2) Right Alt             |
/// | `Function`    | Fn / Globe (held)           | Not available — ignored            |
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordHotkey {
    CapsLock,
    RightOption,
    Function,
}
