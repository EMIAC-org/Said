//! Deterministic mobile contracts shared by the iOS client and gateway tests.
//!
//! This crate intentionally avoids platform APIs, networking, databases, audio,
//! and desktop runtime code. It is safe for the workspace because it only owns
//! schemas, enums, insertion policy, and lightweight validation helpers.

pub mod bridge;
pub mod events;
pub mod fixtures;
pub mod insertion_policy;
pub mod script_guard;
pub mod vocab;
pub mod voice_contract;

pub use bridge::*;
pub use events::*;
pub use insertion_policy::*;
pub use script_guard::*;
pub use vocab::*;
pub use voice_contract::*;
