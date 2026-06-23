//! Shared dictation-polish primitives used by BOTH the local backend
//! (`crates/backend`) and the control-plane server (`crates/control-plane`).
//!
//! These are the quality-critical, drift-prone pieces of the polish pipeline —
//! kept here in `said-core` so both runtimes execute exactly one copy and can
//! never silently diverge again.
//!
//! Everything here is pure (no rusqlite, no sqlx, no AppState) so the
//! workspace-excluded control-plane crate can depend on it freely.

pub mod model;
pub mod prompt;
pub mod script;
pub mod types;
