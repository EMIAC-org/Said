//! DeepSeek profile updater — canonical learn-from-edit path.
//!
//! DeepSeek + Rust validator always produce a HITL proposal first. The profile
//! row changes only after the user approves that proposal.
//!
//! Runtime prompt injection is first-class: approved profiles are used by
//! polish without an environment gate.
//! Model override: `DEEPSEEK_PROFILE_UPDATE_MODEL` (default `deepseek-v4-flash`).

pub mod batch;
pub mod batch_run;
pub mod deepseek;
pub mod jobs;
pub mod prompt;
pub mod run_resolve;
pub mod types;
pub mod validator;
pub mod worker;

pub use types::{LearnFromEditRequest, LearnFromEditResponse, ProfileUpdateRequest};
pub use validator::{ValidatorDecision, validate_and_merge};
