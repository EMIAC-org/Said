//! Backend prompt builder — a thin adapter over the shared
//! [`said_core::polish::prompt`] for the text helpers and voice repair.
//!
//! Dictation polish does not come through here: its prompt is
//! `said_core::polish::dictation`, built on the server.

use crate::store::prefs::Preferences;

pub use said_core::polish::prompt::{
    build_message_polish_system_prompt, build_message_polish_user_message,
    build_refine_last_transform_prompt, build_refine_last_transform_user_message,
    build_tray_format_user_message, build_tray_system_prompt, build_tray_user_message,
    build_user_message, build_voice_repair_system_prompt, build_voice_repair_user_message,
};
pub use said_core::polish::types::PolishPrefs;

fn to_polish_prefs(p: &Preferences) -> PolishPrefs {
    PolishPrefs {
        output_language: p.output_language.clone(),
        tone_preset: p.tone_preset.clone(),
        custom_prompt: p.custom_prompt.clone(),
    }
}

pub fn build_system_prompt(prefs: &Preferences) -> String {
    said_core::polish::prompt::build_system_prompt(&to_polish_prefs(prefs), &[], &[], |_| false)
}

pub fn build_tray_format_system_prompt() -> String {
    said_core::polish::prompt::build_tray_format_system_prompt(&[], &[], |_| false)
}
