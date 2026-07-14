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

pub const MODEL_FILE: &str = "nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf";
pub const MODEL_NAME: &str = "Nemotron Streaming 3.5";
pub const MODEL_SIZE_BYTES: u64 = 751_094_240;
const MODEL_SHA256: &str = "b94545b313b3223fda7b2857a52681da813935c2127643d1e9ff0c23d988089c";
const MODEL_URL: &str = "https://huggingface.co/handy-computer/nemotron-3.5-asr-streaming-0.6b-gguf/resolve/main/nemotron-3.5-asr-streaming-0.6b-Q8_0.gguf";
const DOWNLOAD_EVENT: &str = "nemotron-model-download";

#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
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

static MODEL: OnceLock<Mutex<Option<Model>>> = OnceLock::new();
static DOWNLOAD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<Model>> {
    MODEL.get_or_init(|| Mutex::new(None))
}

fn download_lock() -> &'static Mutex<()> {
    DOWNLOAD_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn model_path() -> PathBuf {
    said_core::paths::data_dir().join("models").join(MODEL_FILE)
}

pub fn installed() -> bool {
    fs::metadata(model_path())
        .map(|metadata| metadata.is_file() && metadata.len() == MODEL_SIZE_BYTES)
        .unwrap_or(false)
}

pub fn unload() {
    if let Ok(mut cached) = cache().lock() {
        *cached = None;
    }
}

fn loaded_model() -> Result<Model, String> {
    if !installed() {
        return Err(format!(
            "{MODEL_NAME} is not installed. Download it in Settings → Speech recognition."
        ));
    }
    let mut cached = cache()
        .lock()
        .map_err(|_| "Nemotron model cache is unavailable".to_string())?;
    if let Some(model) = cached.as_ref() {
        return Ok(model.clone());
    }
    let path = model_path();
    let model =
        Model::load(&path).map_err(|error| format!("Couldn't load {MODEL_NAME}: {error}"))?;
    tracing::info!(
        model = MODEL_FILE,
        backend = %model.backend(),
        architecture = %model.arch(),
        "[nemotron] local model loaded"
    );
    *cached = Some(model.clone());
    Ok(model)
}

/// Best-effort model load performed away from the UI/hotkey thread.
pub fn prewarm() {
    if !installed() {
        return;
    }
    if let Err(error) = loaded_model() {
        tracing::warn!(%error, "[nemotron] prewarm failed");
    }
}

/// Runs batch transcription after a completed Caps-Lock recording.
pub fn transcribe_wav_bytes(wav: &[u8], requested_language: &str) -> Result<Output, String> {
    let started = Instant::now();
    let pcm = asr_core::audio::prepare(wav).map_err(|error| error.to_string())?;
    let model = loaded_model()?;
    let mut session = model
        .session()
        .map_err(|error| format!("Couldn't start {MODEL_NAME}: {error}"))?;
    let options = RunOptions {
        // Auto-detect for Hinglish/auto: forcing either side of a code-switched
        // utterance is exactly the behaviour we need to evaluate before leaving
        // the Experimental label.
        language: language_hint(requested_language),
        ..RunOptions::default()
    };
    let result = session
        .run(&pcm, &options)
        .map_err(|error| format!("{MODEL_NAME} transcription failed: {error}"))?;
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
pub fn nemotron_model_status() -> ModelStatus {
    let path = model_path();
    let size_bytes = fs::metadata(&path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    ModelStatus {
        installed: installed(),
        size_bytes,
        path: path.to_string_lossy().to_string(),
    }
}

#[tauri::command]
pub fn delete_nemotron_model() -> Result<(), String> {
    if said_core::prefs::load().local_stt_model == "nemotron" {
        return Err("Switch dictation back to Oriserve before removing Nemotron.".to_string());
    }
    unload();
    let path = model_path();
    if path.exists() {
        fs::remove_file(&path).map_err(|error| format!("couldn't delete {MODEL_NAME}: {error}"))?;
    }
    let part = path.with_extension("gguf.part");
    let _ = fs::remove_file(part);
    Ok(())
}

#[tauri::command]
pub async fn download_nemotron_model(app: AppHandle) -> Result<(), String> {
    if installed() {
        return Ok(());
    }
    let app_for_task = app.clone();
    tauri::async_runtime::spawn_blocking(move || download_blocking(&app_for_task))
        .await
        .map_err(|error| format!("Nemotron download task failed: {error}"))?
}

fn download_blocking(app: &AppHandle) -> Result<(), String> {
    let _single_download = download_lock()
        .lock()
        .map_err(|_| "Nemotron download registry is unavailable".to_string())?;
    if installed() {
        return Ok(());
    }
    let dest = model_path();
    let dir = dest
        .parent()
        .ok_or_else(|| "Nemotron model path has no parent directory".to_string())?;
    fs::create_dir_all(dir).map_err(|error| format!("couldn't create models folder: {error}"))?;
    let part = dir.join(format!("{MODEL_FILE}.part"));
    let emit = |received: u64, status: &str, error: Option<String>| {
        let _ = app.emit(
            DOWNLOAD_EVENT,
            DownloadProgress {
                name: MODEL_FILE.to_string(),
                received,
                total: MODEL_SIZE_BYTES,
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
        .get(MODEL_URL)
        .send()
        .map_err(|error| fail(format!("Nemotron download request failed: {error}"), 0))?;
    if !response.status().is_success() {
        return Err(fail(
            format!("Nemotron download failed: HTTP {}", response.status()),
            0,
        ));
    }
    if let Some(total) = response.content_length() {
        if total != MODEL_SIZE_BYTES {
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
    if received != MODEL_SIZE_BYTES {
        return Err(fail(
            format!(
                "Nemotron download ended with the wrong size ({received}/{MODEL_SIZE_BYTES} bytes)"
            ),
            received,
        ));
    }
    let checksum = format!("{:x}", hasher.finalize());
    if checksum != MODEL_SHA256 {
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
}
