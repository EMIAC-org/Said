//! `said-paster` — focused-text reader + HID typist for Said.
//!
//! Two responsibilities:
//!   * Read text from the currently focused field (six-strategy fallback stack)
//!   * Synthesize keystrokes or paste-clipboard into the focused field
//!
//! ## Platform layout
//!
//! | Module        | OS gate                  | Status                             |
//! |---------------|--------------------------|------------------------------------|
//! | [`macos`]     | `target_os = "macos"`    | Full AX + CGEvent implementation   |
//! | [`windows`]   | `target_os = "windows"`  | Stub scaffolding (real impl: P3)   |
//!
//! Cross-platform serializable types ([`AxDiagnostics`], [`AxMethodResult`])
//! live in the private [`shared`] module and are re-exported here.

mod shared;
pub use shared::{AxDiagnostics, AxMethodResult};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
compile_error!(
    "said-paster: only macOS and Windows are supported. \
     Linux support is on the roadmap — see the Branching & Release Strategy reference doc."
);
