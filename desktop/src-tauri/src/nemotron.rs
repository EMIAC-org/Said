//! Optional NVIDIA Nemotron 3.5 local dictation provider.
//!
//! This module is intentionally isolated from `asr-core`: Oriserve/Meetings
//! remain Whisper-only, while Nemotron is a different GGUF architecture loaded
//! through `transcribe-cpp`.  The first product slice transcribes the complete
//! recording after the hold key is released; its native partial-stream API is
//! reserved for a future recorder/paster redesign.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use transcribe_cpp::{Model, RunOptions};

pub const MODEL_NAME: &str = "Nemotron Streaming 3.5";
const DOWNLOAD_EVENT: &str = "nemotron-model-download";

/// Downloadable quantizations of the same Nemotron architecture.  Keep this
/// explicit rather than treating the filename as user input: every download is
/// size- and SHA-256-checked before it can be used for dictation.
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

    pub const fn file(self) -> &'static str {
        match self {
            Self::Q4 => "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf",
            Self::Q8 => "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf",
        }
    }

    pub const fn size_bytes(self) -> u64 {
        match self {
            Self::Q4 => 495_831_520,
            Self::Q8 => 751_094_240,
        }
    }

    const fn sha256(self) -> &'static str {
        match self {
            Self::Q4 => "41c99fa5fb6f3d35f68e79adc3e755eca2232a8d921178bd647b71194792b8fd",
            Self::Q8 => "b94545b313b3223fda7b2857a52681da813935c2127643d1e9ff0c23d988089c",
        }
    }

    fn url(self) -> String {
        format!(
            "https://huggingface.co/handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf/resolve/main/{}",
            self.file()
        )
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Q4 => "Nemotron Streaming 3.5 (Q4)",
            Self::Q8 => "Nemotron Streaming 3.5 (Q8)",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub variant: String,
    pub installed: bool,
    pub size_bytes: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
struct DownloadProgress {
    name: String,
    received: u64,
    total: u64,
    status: String,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Output {
    pub transcript: String,
    pub language: Option<String>,
    pub duration_ms: u64,
}

struct CachedModel {
    variant: Variant,
    model: Model,
}

static MODEL: OnceLock<Mutex<Option<CachedModel>>> = OnceLock::new();
static DOWNLOAD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CachedModel>> {
    MODEL.get_or_init(|| Mutex::new(None))
}

fn download_lock() -> &'static Mutex<()> {
    DOWNLOAD_LOCK.get_or_init(|| Mutex::new(()))
}

fn model_path(variant: Variant) -> PathBuf {
    said_core::paths::data_dir()
        .join("models")
        .join(variant.file())
}

pub fn installed(variant: Variant) -> bool {
    fs::metadata(model_path(variant))
        .map(|metadata| metadata.is_file() && metadata.len() == variant.size_bytes())
        .unwrap_or(false)
}

/// `nemotron` was the Q8-only preference in the previous release. Preserve it
/// as Q8 so an existing user never silently changes models after updating.
pub fn selected_variant() -> Option<Variant> {
    selected_variant_for(&said_core::prefs::load().local_stt_model)
}

pub fn selected_variant_for(value: &str) -> Option<Variant> {
    match value {
        "nemotron-q4" => Some(Variant::Q4),
        "nemotron" | "nemotron-q8" => Some(Variant::Q8),
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
    selected_variant().is_some_and(installed)
}

pub fn selected_model_file() -> &'static str {
    selected_variant().unwrap_or(Variant::Q8).file()
}

pub fn selected_model_name() -> &'static str {
    selected_variant().unwrap_or(Variant::Q8).display_name()
}

pub fn unload() {
    if let Ok(mut cached) = cache().lock() {
        *cached = None;
    }
}

fn loaded_model(variant: Variant) -> Result<Model, String> {
    if !installed(variant) {
        return Err(format!(
            "{} is not installed. Download it in Settings → Speech recognition.",
            variant.display_name()
        ));
    }
    let mut cached = cache()
        .lock()
        .map_err(|_| "Nemotron model cache is unavailable".to_string())?;
    if let Some(cached_model) = cached.as_ref()
        && cached_model.variant == variant
    {
        return Ok(cached_model.model.clone());
    }
    // Do not keep Q4 and Q8 mapped together. A user can safely compare them
    // without paying both models' RAM cost.
    *cached = None;
    let path = model_path(variant);
    let model = Model::load(&path)
        .map_err(|error| format!("Couldn't load {}: {error}", variant.display_name()))?;
    tracing::info!(
        model = variant.file(),
        backend = %model.backend(),
        architecture = %model.arch(),
        "[nemotron] local model loaded"
    );
    *cached = Some(CachedModel {
        variant,
        model: model.clone(),
    });
    Ok(model)
}

/// Best-effort model load performed away from the UI/hotkey thread.
pub fn prewarm() {
    let Some(variant) = selected_variant() else {
        return;
    };
    if !installed(variant) {
        return;
    }
    if let Err(error) = loaded_model(variant) {
        tracing::warn!(%error, "[nemotron] prewarm failed");
    }
}

/// Runs batch transcription after a completed Caps-Lock recording.
pub fn transcribe_wav_bytes(wav: &[u8], requested_language: &str) -> Result<Output, String> {
    let started = Instant::now();
    let variant = selected_variant().ok_or_else(|| "Nemotron is not selected.".to_string())?;
    let pcm = asr_core::audio::prepare(wav).map_err(|error| error.to_string())?;
    let model = loaded_model(variant)?;
    let mut session = model
        .session()
        .map_err(|error| format!("Couldn't start {}: {error}", variant.display_name()))?;
    let options = RunOptions {
        // Auto-detect for Hinglish/auto: forcing either side of a code-switched
        // utterance is exactly the behaviour we need to evaluate before leaving
        // the Experimental label.
        language: language_hint(requested_language),
        ..RunOptions::default()
    };
    let result = session
        .run(&pcm, &options)
        .map_err(|error| format!("{} transcription failed: {error}", variant.display_name()))?;
    let transcript = strip_terminal_language_tag(result.text).trim().to_string();
    if transcript.is_empty() {
        return Err("No speech detected — try speaking again.".to_string());
    }
    Ok(Output {
        transcript,
        language: result.language,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn language_hint(language: &str) -> Option<String> {
    match language.trim().to_ascii_lowercase().as_str() {
        "en" | "english" => Some("en-US".to_string()),
        "hi" | "hindi" => Some("hi-IN".to_string()),
        _ => None,
    }
}

fn strip_terminal_language_tag(text: String) -> String {
    let trimmed = text.trim_end();
    let Some(start) = trimmed.rfind("<") else {
        return trimmed.to_string();
    };
    let candidate = &trimmed[start..];
    let is_language_tag = candidate.len() == 7
        && candidate.starts_with('<')
        && candidate.ends_with('>')
        && candidate.as_bytes()[3] == b'-';
    if is_language_tag {
        trimmed[..start].trim_end().to_string()
    } else {
        trimmed.to_string()
    }
}

#[tauri::command]
pub fn nemotron_model_status(variant: String) -> Result<ModelStatus, String> {
    let variant = Variant::parse(&variant)?;
    let path = model_path(variant);
    let size_bytes = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    Ok(ModelStatus {
        variant: variant.key().to_string(),
        installed: installed(variant),
        size_bytes,
        path: path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn delete_nemotron_model(variant: String) -> Result<(), String> {
    let variant = Variant::parse(&variant)?;
    let prefs = said_core::prefs::load();
    if prefs.dictation_stt == crate::stt_policy::LOCAL_PREF
        && selected_variant_for(&prefs.local_stt_model) == Some(variant)
    {
        return Err(
            "Switch dictation to Oriserve or the other Nemotron model before removing this model."
                .to_string(),
        );
    }
    unload();
    let path = model_path(variant);
    if path.exists() {
        fs::remove_file(&path).map_err(|error| format!("couldn't delete {MODEL_NAME}: {error}"))?;
    }
    let part = path.with_extension("gguf.part");
    let _ = fs::remove_file(part);
    Ok(())
}

#[tauri::command]
pub async fn download_nemotron_model(app: AppHandle, variant: String) -> Result<(), String> {
    let variant = Variant::parse(&variant)?;
    if installed(variant) {
        return Ok(());
    }
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || download_blocking(&app_for_task, variant))
        .await
        .map_err(|error| format!("Nemotron download task failed: {error}"))?
}

fn download_blocking(app: &AppHandle, variant: Variant) -> Result<(), String> {
    let _single_download = download_lock()
        .lock()
        .map_err(|_| "Nemotron download registry is unavailable".to_string())?;
    if installed(variant) {
        return Ok(());
    }
    let dest = model_path(variant);
    let dir = dest
        .parent()
        .ok_or_else(|| "Nemotron model path has no parent directory".to_string())?;
    fs::create_dir_all(dir).map_err(|error| format!("couldn't create models folder: {error}"))?;
    let part = dir.join(format!("{}.part", variant.file()));
    let emit = |received: u64, status: &str, error: Option<String>| {
        let _ = app.emit(
            DOWNLOAD_EVENT,
            DownloadProgress {
                name: variant.file().to_string(),
                received,
                total: variant.size_bytes(),
                status: status.to_string(),
                error,
            },
        );
    };
    let fail = |message: String, received: u64| -> String {
        let _ = fs::remove_file(&part);
        emit(received, "error", Some(message.clone()));
        message
    };

    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|error| fail(format!("Nemotron HTTP client failed: {error}"), 0))?;
    let mut response = client
        .get(variant.url())
        .send()
        .map_err(|error| fail(format!("Nemotron download request failed: {error}"), 0))?;
    if !response.status().is_success() {
        return Err(fail(
            format!("Nemotron download failed: HTTP {}", response.status()),
            0,
        ));
    }
    if let Some(total) = response.content_length() {
        if total != variant.size_bytes() {
            return Err(fail(
                format!("Nemotron download has an unexpected size ({total} bytes)"),
                0,
            ));
        }
    }

    let mut file = fs::File::create(&part).map_err(|error| {
        fail(
            format!("couldn't create Nemotron temporary file: {error}"),
            0,
        )
    })?;
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut hasher = Sha256::new();
    let mut received = 0_u64;
    let mut last_emitted = 0_u64;
    emit(0, "downloading", None);
    loop {
        let count = response
            .read(&mut buffer)
            .map_err(|error| fail(format!("Nemotron download read failed: {error}"), received))?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])
            .map_err(|error| fail(format!("Nemotron download write failed: {error}"), received))?;
        hasher.update(&buffer[..count]);
        received += count as u64;
        if received.saturating_sub(last_emitted) >= 2_000_000 {
            last_emitted = received;
            emit(received, "downloading", None);
        }
    }
    file.flush().ok();
    let _ = file.sync_all();
    drop(file);
    if received != variant.size_bytes() {
        return Err(fail(
            format!(
                "Nemotron download ended with the wrong size ({received}/{} bytes)",
                variant.size_bytes()
            ),
            received,
        ));
    }
    let checksum = format!("{:x}", hasher.finalize());
    if checksum != variant.sha256() {
        return Err(fail(
            "Nemotron download failed its integrity check. Please try again.".to_string(),
            received,
        ));
    }
    fs::rename(&part, &dest).map_err(|error| {
        fail(
            format!("couldn't finalize Nemotron model: {error}"),
            received,
        )
    })?;
    unload();
    emit(received, "done", None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_explicit_hints_only_for_single_language_modes() {
        assert_eq!(language_hint("english"), Some("en-US".to_string()));
        assert_eq!(language_hint("hindi"), Some("hi-IN".to_string()));
        assert_eq!(language_hint("hinglish"), None);
        assert_eq!(language_hint("auto"), None);
    }

    #[test]
    fn removes_only_a_terminal_language_tag() {
        assert_eq!(
            strip_terminal_language_tag("Hello bhai. <en-US>".to_string()),
            "Hello bhai."
        );
        assert_eq!(
            strip_terminal_language_tag("hello <not-a-tag>".to_string()),
            "hello <not-a-tag>"
        );
    }

    #[test]
    fn q4_and_q8_metadata_are_explicit_and_legacy_q8_stays_stable() {
        assert_eq!(
            Variant::Q4.file(),
            "nemotron-3.5-asr-streaming-0.6b-Q4_K_M.gguf"
        );
        assert_eq!(Variant::Q4.size_bytes(), 495_831_520);
        assert_eq!(Variant::Q8.size_bytes(), 751_094_240);
        assert_eq!(selected_variant_for("nemotron-q4"), Some(Variant::Q4));
        assert_eq!(selected_variant_for("nemotron-q8"), Some(Variant::Q8));
        assert_eq!(selected_variant_for("nemotron"), Some(Variant::Q8));
        assert_eq!(selected_variant_for("oriserve"), None);
    }
}
