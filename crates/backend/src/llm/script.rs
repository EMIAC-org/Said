//! Script utilities for enforcing output-language contracts.
//!
//! The implementation now lives in [`said_core::polish::script`] so the local
//! backend and the control-plane server share exactly one Devanagari→Roman
//! guard and can never drift apart. This module is a thin re-export.

pub use said_core::polish::script::*;
