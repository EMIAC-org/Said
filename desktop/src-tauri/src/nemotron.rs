//! Compatibility facade for released Nemotron commands and preference keys.
//!
//! Catalog metadata, storage, and transcribe.cpp ownership now live in the
//! generic local-model modules. Keep this facade until released clients and
//! meeting code no longer call the Nemotron-specific command names.

use serde::Serialize;
use tauri::AppHandle;

use crate::local_model_catalog::{self, LocalModelDescriptor};

pub const MODEL_NAME: &str = "Nemotron Streaming 3.5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Q4,
    Q8,
}

impl Variant {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "q4" => Ok(Self::Q4),
            "q8" => Ok(Self::Q8),
            _ => Err("Unknown Nemotron model. Choose Q4 or Q8.".to_string()),
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Q4 => "q4",
            Self::Q8 => "q8",
        }
    }

    pub const fn pref(self) -> &'static str {
        match self {
            Self::Q4 => local_model_catalog::NEMOTRON_Q4_PREF,
            Self::Q8 => local_model_catalog::NEMOTRON_Q8_PREF,
        }
    }

    fn descriptor(self) -> &'static LocalModelDescriptor {
        local_model_catalog::find(self.pref()).expect("Nemotron catalog descriptor")
    }

    pub fn file(self) -> &'static str {
        self.descriptor().filename
    }

    pub fn size_bytes(self) -> u64 {
        self.descriptor().size_bytes
    }

    pub fn display_name(self) -> &'static str {
        self.descriptor().name
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub variant: String,
    pub installed: bool,
    pub size_bytes: u64,
    pub path: String,
}

pub type Output = crate::local_transcribe::Output;

pub fn installed(variant: Variant) -> bool {
    crate::local_model_store::installed(variant.descriptor())
}

pub fn selected_variant() -> Option<Variant> {
    selected_variant_for(&said_core::prefs::load().local_stt_model)
}

pub fn selected_variant_for(value: &str) -> Option<Variant> {
    match value {
        local_model_catalog::NEMOTRON_Q4_PREF => Some(Variant::Q4),
        "nemotron" | local_model_catalog::NEMOTRON_Q8_PREF => Some(Variant::Q8),
        _ => None,
    }
}

pub fn is_selected() -> bool {
    selected_variant().is_some()
}

pub fn is_nemotron_pref(value: &str) -> bool {
    selected_variant_for(value).is_some()
}

pub fn selected_installed() -> bool {
    crate::local_transcribe::selected_installed()
}

pub fn selected_model_file() -> &'static str {
    crate::local_transcribe::selected_model_file()
}

pub fn selected_model_name() -> &'static str {
    crate::local_transcribe::selected_model_name()
}

pub fn unload() {
    crate::local_transcribe::unload();
}

pub fn prewarm() {
    crate::local_transcribe::prewarm();
}

pub fn transcribe_wav_bytes(wav: &[u8], requested_language: &str) -> Result<Output, String> {
    crate::local_transcribe::transcribe_wav_bytes(wav, requested_language, None)
}

pub fn transcribe_wav_bytes_for(
    variant: Variant,
    wav: &[u8],
    requested_language: &str,
) -> Result<Output, String> {
    crate::local_transcribe::transcribe_wav_bytes_for_key(variant.pref(), wav, requested_language)
}

#[tauri::command]
pub fn nemotron_model_status(variant: String) -> Result<ModelStatus, String> {
    let variant = Variant::parse(&variant)?;
    let descriptor = variant.descriptor();
    let path = crate::local_model_store::model_path(descriptor);
    Ok(ModelStatus {
        variant: variant.key().to_string(),
        installed: crate::local_model_store::installed(descriptor),
        size_bytes: crate::local_model_store::installed_size(descriptor),
        path: path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub async fn download_nemotron_model(app: AppHandle, variant: String) -> Result<(), String> {
    let variant = Variant::parse(&variant)?;
    crate::local_model_store::download_local_model(app, variant.pref().to_string()).await
}

#[tauri::command]
pub fn delete_nemotron_model(variant: String) -> Result<(), String> {
    let variant = Variant::parse(&variant)?;
    let prefs = said_core::prefs::load();
    if prefs.dictation_stt == crate::stt_policy::LOCAL_PREF
        && selected_variant_for(&prefs.local_stt_model) == Some(variant)
    {
        return Err(
            "Switch dictation to another installed model before removing this model.".to_string(),
        );
    }
    unload();
    crate::local_model_store::remove(variant.descriptor()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_keys_and_artifacts_remain_stable() {
        assert_eq!(
            Variant::Q4.file(),
            "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf"
        );
        assert_eq!(Variant::Q4.size_bytes(), 495_831_520);
        assert_eq!(selected_variant_for("nemotron-q4"), Some(Variant::Q4));
        assert_eq!(selected_variant_for("nemotron-q8"), Some(Variant::Q8));
        assert_eq!(selected_variant_for("nemotron"), Some(Variant::Q8));
        assert_eq!(selected_variant_for("oriserve"), None);
    }
}
